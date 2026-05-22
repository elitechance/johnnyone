const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'http://127.0.0.1:4200';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR
  || path.resolve(__dirname, '../../../../docs/johnnyone/plans/mobile-terminal-experience/phases/03-validation/tasks/02-cross-client-regression/artifacts');
const TENANT_ID = process.env.JOHNNYONE_E2E_TENANT_ID || '00000000-0000-0000-0000-000000000001';

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function now() {
  return new Date().toISOString();
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function createSession(id) {
  return {
    id,
    title: 'Cross Client Terminal',
    provider: 'ollama',
    model: 'qwen3.5:2b',
    workingDirectory: '/home/creepy/documents/workspace',
    status: 'active',
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCostCents: 0,
    createdAt: now(),
    updatedAt: now(),
  };
}

function createRun(eventType = 'phase_started') {
  const updatedAt = now();
  const plan = {
    id: 'cross-client-development-plan',
    runType: 'development',
    title: 'Cross Client Development',
    workspacePath: '/home/creepy/documents/workspace',
    planPath: 'personal/docs/johnnyone/plans/mobile-terminal-experience',
    status: 'running',
    workerSessionId: 'cross-worker-session',
    reviewerSessionId: 'cross-reviewer-session',
    workerProvider: 'codex',
    reviewerProvider: 'codex',
    currentPhaseId: '03-validation',
    currentPhaseIndex: 0,
    error: null,
    brief: '',
    appScope: 'personal/apps',
    docsScope: 'personal/docs',
    referencePaths: '',
    createdAt: updatedAt,
    updatedAt,
  };
  return {
    plan,
    phases: [
      {
        id: 'cross-phase-1',
        planId: plan.id,
        phaseId: '03-validation',
        phaseTitle: 'Validation',
        phaseIndex: 0,
        status: 'in-progress',
        workerStartedAt: updatedAt,
        workerIdleAt: null,
        reviewerStartedAt: null,
        reviewerIdleAt: null,
        gateVerdict: eventType === 'reviewer_rerun_requested' ? 'rerun-requested' : 'needs-review',
        clarificationAttempts: 5,
        summary: 'Cross-client mobile regression phase',
        findingsJson: '[]',
        createdAt: updatedAt,
        updatedAt,
      },
    ],
    tasks: [
      {
        id: 'cross-task-1',
        planId: plan.id,
        phaseId: '03-validation',
        taskId: '02-cross-client-regression',
        taskTitle: 'Cross client regression',
        taskIndex: 0,
        promptPath: 'personal/docs/johnnyone/plans/mobile-terminal-experience/phases/03-validation/tasks/02-cross-client-regression/prompt.md',
        statusPath: 'personal/docs/johnnyone/plans/mobile-terminal-experience/phases/03-validation/tasks/02-cross-client-regression/status.yml',
        decisionsPath: 'personal/docs/johnnyone/plans/mobile-terminal-experience/phases/03-validation/tasks/02-cross-client-regression/decisions.md',
        status: 'in-progress',
        createdAt: updatedAt,
        updatedAt,
      },
    ],
    events: [
      {
        id: `cross-event-${eventType}`,
        planId: plan.id,
        phaseId: '03-validation',
        eventType,
        payloadJson: JSON.stringify({ source: 'mobile-cross-client.e2e', propagated: true }),
        createdAt: updatedAt,
      },
    ],
  };
}

async function screenshot(page, name) {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const filePath = path.join(ARTIFACT_DIR, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: false });
  console.log(`mobile-cross-client.e2e: screenshot ${filePath}`);
}

async function primeAuthAndRelay(page, label) {
  await page.evaluateOnNewDocument((tenantId, pageLabel) => {
    const user = {
      id: `cross-client-${pageLabel}`,
      tenantId,
      email: 'admin@johnnyone.local',
      displayName: 'Cross Client E2E',
      roles: ['admin'],
      status: 'active',
    };
    window.localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    window.localStorage.setItem('johnnyone_access_token', 'cross-client-e2e-token');
    window.localStorage.setItem('johnnyone_refresh_token', 'cross-client-e2e-refresh-token');
    window.localStorage.setItem('johnnyone_tenant_id', tenantId);
    window.localStorage.setItem('johnnyone_user_id', user.id);
    window.localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));

    const sockets = [];
    const relayChannel = new BroadcastChannel('johnnyone-cross-client-relay');

    function screenFor(sessionId) {
      const role = sessionId.includes('reviewer') ? 'T2 reviewer' : sessionId.includes('worker') ? 'T1 worker' : 'terminal';
      return {
        sessionId,
        status: 'attached',
        cursor: Date.now(),
        content: `JohnnyOne ${role} mock\n${sessionId}\n>>> `,
        cols: 80,
        rows: 24,
        tmuxSessionName: `tmux-${sessionId}`,
        paneId: '%1',
      };
    }

    function deliver(type, data) {
      for (const socket of sockets) {
        socket.onmessage?.({ data: JSON.stringify({ type, data }) });
      }
    }

    relayChannel.onmessage = (event) => {
      const message = event.data || {};
      if (message.type) deliver(message.type, message.data);
    };

    window.__johnnyoneRelayBroadcast = (type, data) => {
      deliver(type, data);
      relayChannel.postMessage({ type, data });
    };

    class MockRelayWebSocket {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSING = 2;
      static CLOSED = 3;

      constructor() {
        this.readyState = MockRelayWebSocket.CONNECTING;
        sockets.push(this);
        setTimeout(() => {
          this.readyState = MockRelayWebSocket.OPEN;
          this.onopen?.({ type: 'open' });
        }, 0);
      }

      send(raw) {
        const envelope = JSON.parse(raw);
        const data = envelope.data || {};
        if (envelope.type === 'terminal_command') {
          if (data.sessionId && (data.control || data.data)) {
            this.onmessage?.({
              data: JSON.stringify({ type: 'terminal_screen', data: screenFor(data.sessionId) }),
            });
          }
          this.onmessage?.({
            data: JSON.stringify({
              type: 'terminal_command_ack',
              data: { requestId: data.requestId, sessionId: data.sessionId, accepted: true },
            }),
          });
        }
        if (envelope.type === 'terminal_resize' || envelope.type === 'terminal_kill') {
          this.onmessage?.({
            data: JSON.stringify({
              type: 'terminal_command_ack',
              data: { requestId: data.requestId, sessionId: data.sessionId, accepted: true },
            }),
          });
        }
      }

      close() {
        this.readyState = MockRelayWebSocket.CLOSED;
        this.onclose?.({ type: 'close', wasClean: true });
      }

      addEventListener(type, handler) {
        this[`on${type}`] = handler;
      }

      removeEventListener(type) {
        this[`on${type}`] = null;
      }
    }

    window.WebSocket = MockRelayWebSocket;
  }, TENANT_ID, label);
}

async function broadcast(page, type, data) {
  await page.evaluate((eventType, eventData) => {
    window.__johnnyoneRelayBroadcast?.(eventType, eventData);
  }, type, data);
}

async function mockGraphql(page, state) {
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

    const body = request.postData() || '{}';
    const parsed = JSON.parse(body);
    const query = parsed.query || body;
    const variables = parsed.variables || {};
    let data = {};
    let relayEvent = null;

    if (query.includes('listAiSessions')) {
      data = { listAiSessions: state.sessions.filter((session) => session.status === 'active') };
    } else if (query.includes('createAiSession')) {
      const session = createSession(`cross-session-${state.sessions.length + 1}`);
      state.sessions.unshift(session);
      data = { createAiSession: session };
      relayEvent = { type: 'session_updated', data: { sessionId: session.id, session } };
    } else if (query.includes('getAiSession')) {
      data = { getAiSession: state.sessions.find((session) => session.id === variables.id) || state.sessions[0] };
    } else if (query.includes('listAiMessages')) {
      data = { listAiMessages: [] };
    } else if (query.includes('updateAiSessionArchived')) {
      const session = state.sessions.find((item) => item.id === variables.id) || createSession(variables.id);
      session.status = 'archived';
      session.updatedAt = now();
      data = { updateAiSessionArchived: session };
      relayEvent = { type: 'session_deleted', data: { sessionId: session.id } };
    } else if (query.includes('listDetectedCliTools')) {
      data = { listDetectedCliTools: [{ provider: 'ollama', command: 'ollama', found: true, path: '/usr/bin/ollama' }] };
    } else if (query.includes('getSetting')) {
      data = { getSetting: '/home/creepy/documents/workspace' };
    } else if (query.includes('listDesktopNodes')) {
      data = {
        listDesktopNodes: [{
          id: 'mock-node',
          hostname: 'mock-host',
          os: 'linux',
          arch: 'x64',
          version: 'e2e',
          status: 'online',
          capabilities: [],
          lastHeartbeatAt: now(),
          createdAt: now(),
        }],
      };
    } else if (query.includes('listAgentPlans')) {
      data = { listAgentPlans: [state.run.plan] };
    } else if (query.includes('getAgentPlan')) {
      data = { getAgentPlan: state.run };
    } else if (query.includes('retryAgentReviewer')) {
      state.run = createRun('reviewer_rerun_requested');
      data = { retryAgentReviewer: state.run };
      relayEvent = {
        type: 'agent_plan_run_updated',
        data: { planId: state.run.plan.id, deleted: false, run: state.run },
      };
    } else if (query.includes('getPlannerPromptSettings')) {
      data = {
        getPlannerPromptSettings: {
          schema: 'planner-prompt-settings/v1',
          development: { worker: '', reviewer: '' },
          planning: { planner: '', reviewer: '' },
        },
      };
    } else if (query.includes('validateWorkspacePlan')) {
      data = {
        validateWorkspacePlan: {
          valid: true,
          workspacePath: '/home/creepy/documents/workspace',
          planPath: 'personal/docs/johnnyone/plans/mobile-terminal-experience',
          title: 'Mobile Terminal Experience',
          phaseCount: 4,
          taskCount: 8,
          error: null,
        },
      };
    }

    request.respond({
      status: 200,
      contentType: 'application/json',
      headers: { 'Access-Control-Allow-Origin': '*' },
      body: JSON.stringify({ data }),
    }).then(() => {
      if (relayEvent) {
        setTimeout(() => {
          broadcast(page, relayEvent.type, relayEvent.data).catch((error) => {
            console.error(`mobile-cross-client.e2e: broadcast failed ${error.message}`);
          });
        }, 50);
      }
    });
  });
}

async function openTerminal(page, label) {
  page.on('pageerror', (error) => console.error(`[${label}:pageerror] ${error.message}`));
  console.log(`mobile-cross-client.e2e: open ${label} terminal`);
  await page.goto(`${APP_URL.replace(/\/$/, '')}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
  await page.waitForSelector('.workspace-shell', { timeout: 30000 });
}

async function openDevelopment(page, label) {
  page.on('pageerror', (error) => console.error(`[${label}:pageerror] ${error.message}`));
  console.log(`mobile-cross-client.e2e: open ${label} development`);
  await page.goto(`${APP_URL.replace(/\/$/, '')}/development`, { waitUntil: 'networkidle2', timeout: 60000 });
  await page.waitForSelector('.planner-shell', { timeout: 30000 });
  await page.waitForSelector('.mobile-panel-switcher', { timeout: 30000 });
}

async function count(page, selector) {
  return page.$$eval(selector, (elements) => elements.length);
}

async function paneRect(page) {
  return page.$eval('.terminal-pane.active', (element) => {
    const box = element.getBoundingClientRect();
    return { left: box.left, top: box.top, width: box.width, height: box.height };
  });
}

async function assertDesktopPaneDragResize(page) {
  const beforeDrag = await paneRect(page);
  await page.evaluate(() => {
    const header = document.querySelector('.terminal-pane.active .pane-header');
    const box = header.getBoundingClientRect();
    const event = (type, x, y) => new PointerEvent(type, {
      bubbles: true,
      cancelable: true,
      pointerId: 1,
      pointerType: 'mouse',
      button: 0,
      buttons: type === 'pointerup' ? 0 : 1,
      clientX: x,
      clientY: y,
    });
    header.dispatchEvent(event('pointerdown', box.left + 80, box.top + 18));
    window.dispatchEvent(event('pointermove', box.left + 150, box.top + 68));
    window.dispatchEvent(event('pointerup', box.left + 150, box.top + 68));
  });
  await delay(250);
  const afterDrag = await paneRect(page);
  assert(afterDrag.left > beforeDrag.left + 40, `desktop pane did not drag horizontally: ${JSON.stringify({ beforeDrag, afterDrag })}`);
  assert(afterDrag.top > beforeDrag.top + 30, `desktop pane did not drag vertically: ${JSON.stringify({ beforeDrag, afterDrag })}`);

  await page.evaluate(() => {
    const handle = document.querySelector('.terminal-pane.active .pane-resize-handle');
    const box = handle.getBoundingClientRect();
    const event = (type, x, y) => new PointerEvent(type, {
      bubbles: true,
      cancelable: true,
      pointerId: 2,
      pointerType: 'mouse',
      button: 0,
      buttons: type === 'pointerup' ? 0 : 1,
      clientX: x,
      clientY: y,
    });
    handle.dispatchEvent(event('pointerdown', box.left + 4, box.top + 4));
    window.dispatchEvent(event('pointermove', box.left + 96, box.top + 66));
    window.dispatchEvent(event('pointerup', box.left + 96, box.top + 66));
  });
  await delay(250);
  const afterResize = await paneRect(page);
  assert(afterResize.width > afterDrag.width + 60, `desktop pane did not resize wider: ${JSON.stringify({ afterDrag, afterResize })}`);
  assert(afterResize.height > afterDrag.height + 40, `desktop pane did not resize taller: ${JSON.stringify({ afterDrag, afterResize })}`);
}

async function run() {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const state = { sessions: [], run: createRun() };
  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    const desktop = await browser.newPage();
    const mobile = await browser.newPage();
    await desktop.setViewport({ width: 1440, height: 900 });
    await mobile.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true });
    await primeAuthAndRelay(desktop, 'desktop');
    await primeAuthAndRelay(mobile, 'mobile');
    await mockGraphql(desktop, state);
    await mockGraphql(mobile, state);

    await openTerminal(desktop, 'desktop-terminal');
    await openTerminal(mobile, 'mobile-terminal');
    const desktopTabsBefore = await count(desktop, '.workspace-tab');
    const mobileTabsBefore = await count(mobile, '.workspace-tab');

    console.log('mobile-cross-client.e2e: create desktop terminal');
    await desktop.evaluate(() => {
      document.querySelector('.workspace-actions .primary-action')?.click();
    });
    console.log('mobile-cross-client.e2e: wait desktop terminal');
    await desktop.waitForFunction((before) => document.querySelectorAll('.workspace-tab').length > before, { timeout: 30000 }, desktopTabsBefore);
    await desktop.waitForSelector('.terminal-pane.active .pane-status:not(.offline)', { timeout: 30000 });
    const createdSession = state.sessions.find((session) => session.status === 'active');
    assert(createdSession, 'desktop create did not add a session to shared state');
    await broadcast(mobile, 'session_updated', { sessionId: createdSession.id, session: createdSession });
    console.log('mobile-cross-client.e2e: wait mobile terminal propagation');
    await mobile.waitForFunction((before) => document.querySelectorAll('.workspace-tab').length > before, { timeout: 30000 }, mobileTabsBefore);
    await mobile.evaluate(() => {
      document.querySelector('.workspace-tab')?.click();
    });
    await mobile.waitForSelector('.terminal-pane.active', { timeout: 30000 });
    await screenshot(desktop, '03-02-desktop-terminal-created-before-close');
    await screenshot(mobile, '03-02-390x844-mobile-terminal-propagated-before-close');

    await assertDesktopPaneDragResize(desktop);
    await screenshot(desktop, '03-02-desktop-terminal-drag-resize');

    const desktopTabsAfterCreate = await count(desktop, '.workspace-tab');
    const mobileTabsAfterCreate = await count(mobile, '.workspace-tab');
    console.log('mobile-cross-client.e2e: propagate terminal close');
    await broadcast(desktop, 'session_deleted', { sessionId: createdSession.id });
    await broadcast(mobile, 'session_deleted', { sessionId: createdSession.id });
    await desktop.waitForFunction((before) => document.querySelectorAll('.workspace-tab').length === before - 1, { timeout: 30000 }, desktopTabsAfterCreate);
    await mobile.waitForFunction((before) => document.querySelectorAll('.workspace-tab').length === before - 1, { timeout: 30000 }, mobileTabsAfterCreate);
    await desktop.goto(`${APP_URL.replace(/\/$/, '')}/terminal`, { waitUntil: 'networkidle2', timeout: 60000 });
    await desktop.waitForSelector('.workspace-shell', { timeout: 30000 });
    await screenshot(desktop, '03-02-desktop-terminal-closed-after-close');
    await screenshot(mobile, '03-02-390x844-mobile-terminal-removed-after-close');

    await openDevelopment(desktop, 'desktop-planner');
    await openDevelopment(mobile, 'mobile-planner');
    await mobile.evaluate(() => {
      const buttons = Array.from(document.querySelectorAll('.mobile-panel-switcher button'));
      buttons.find((button) => (button.textContent || '').trim() === 'Coordinator')?.click();
    });
    await mobile.waitForSelector('.coordinator-panel.active-mobile-panel', { timeout: 15000 });
    await screenshot(mobile, '03-02-390x844-mobile-planner-before-rerun');

    console.log('mobile-cross-client.e2e: send planner rerun from desktop');
    await desktop.evaluate(() => {
      const buttons = Array.from(document.querySelectorAll('.coordinator-actions button'));
      buttons.find((button) => (button.textContent || '').trim() === 'Rerun T2')?.click();
    });
    state.run = createRun('reviewer_rerun_requested');
    await broadcast(mobile, 'agent_plan_run_updated', {
      planId: state.run.plan.id,
      deleted: false,
      run: state.run,
    });
    console.log('mobile-cross-client.e2e: wait mobile planner propagation');
    await mobile.waitForFunction(
      () => (document.querySelector('.event-log')?.textContent || '').includes('reviewer_rerun_requested'),
      { timeout: 30000 },
    );
    await screenshot(desktop, '03-02-desktop-planner-rerun-sent');
    await screenshot(mobile, '03-02-390x844-mobile-planner-rerun-propagated');

    console.log('mobile-cross-client.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
