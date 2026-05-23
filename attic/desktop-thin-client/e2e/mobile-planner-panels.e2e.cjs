const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'http://127.0.0.1:4200';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR || null;
const TASKS_DIR = process.env.JOHNNYONE_PHASE02_TASKS_DIR
  || path.resolve(__dirname, '../../../../docs/johnnyone/plans/mobile-terminal-experience/phases/02-planning-development-mobile/tasks');
const TENANT_ID = process.env.JOHNNYONE_E2E_TENANT_ID || '00000000-0000-0000-0000-000000000001';

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function taskArtifactDir(taskId) {
  if (ARTIFACT_DIR) return ARTIFACT_DIR;
  return path.join(TASKS_DIR, taskId, 'artifacts');
}

async function screenshot(page, taskId, name) {
  const dir = taskArtifactDir(taskId);
  fs.mkdirSync(dir, { recursive: true });
  const filePath = path.join(dir, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: true });
  console.log(`mobile-planner-panels.e2e: screenshot ${filePath}`);
}

function now() {
  return new Date().toISOString();
}

function createPlan(runType) {
  return {
    id: `${runType}-plan`,
    runType,
    title: runType === 'planning' ? 'Mobile Planning Run' : 'Mobile Development Run',
    workspacePath: '/home/creepy/documents/workspace',
    planPath: 'personal/docs/johnnyone/plans/mobile-terminal-experience',
    status: 'running',
    workerSessionId: `${runType}-worker-session`,
    reviewerSessionId: `${runType}-reviewer-session`,
    workerProvider: 'codex',
    reviewerProvider: 'codex',
    currentPhaseId: runType === 'planning' ? '00-discovery' : '01-terminal-mobile',
    currentPhaseIndex: 0,
    error: null,
    brief: runType === 'planning' ? 'Plan a mobile terminal experience.' : '',
    appScope: 'personal/apps',
    docsScope: 'personal/docs',
    referencePaths: '',
    createdAt: now(),
    updatedAt: now(),
  };
}

function createRun(runType) {
  const plan = createPlan(runType);
  const phases = [
    {
      id: `${runType}-phase-1`,
      planId: plan.id,
      phaseId: plan.currentPhaseId,
      phaseTitle: runType === 'planning' ? 'Discovery' : 'Terminal Mobile',
      phaseIndex: 0,
      status: 'in-progress',
      workerStartedAt: now(),
      workerIdleAt: null,
      reviewerStartedAt: null,
      reviewerIdleAt: null,
      gateVerdict: runType === 'development' ? 'needs-review' : 'pending',
      clarificationAttempts: runType === 'development' ? 5 : 0,
      summary: 'Mobile e2e phase summary',
      findingsJson: '[]',
      createdAt: now(),
      updatedAt: now(),
    },
    {
      id: `${runType}-phase-2`,
      planId: plan.id,
      phaseId: runType === 'planning' ? '01-draft' : '02-planning-development-mobile',
      phaseTitle: runType === 'planning' ? 'Draft Plan' : 'Planning Development Mobile',
      phaseIndex: 1,
      status: 'pending',
      workerStartedAt: null,
      workerIdleAt: null,
      reviewerStartedAt: null,
      reviewerIdleAt: null,
      gateVerdict: 'pending',
      clarificationAttempts: 0,
      summary: '',
      findingsJson: '[]',
      createdAt: now(),
      updatedAt: now(),
    },
  ];
  const tasks = phases.flatMap((phase, index) => [
    {
      id: `${phase.id}-task-1`,
      planId: plan.id,
      phaseId: phase.phaseId,
      taskId: `${String(index + 1).padStart(2, '0')}-mobile-panel`,
      taskTitle: 'Mobile panel validation',
      taskIndex: 0,
      promptPath: `${plan.planPath}/phases/${phase.phaseId}/tasks/01/prompt.md`,
      statusPath: `${plan.planPath}/phases/${phase.phaseId}/tasks/01/status.yml`,
      decisionsPath: `${plan.planPath}/phases/${phase.phaseId}/tasks/01/decisions.md`,
      status: index === 0 ? 'in-progress' : 'not-started',
      createdAt: now(),
      updatedAt: now(),
    },
  ]);
  return {
    plan,
    phases,
    tasks,
    events: [
      {
        id: `${runType}-event-1`,
        planId: plan.id,
        phaseId: plan.currentPhaseId,
        eventType: 'mobile_e2e_ready',
        payloadJson: JSON.stringify({ route: runType, panel: 'coordinator' }),
        createdAt: now(),
      },
    ],
  };
}

async function primeAuthAndRelay(page) {
  await page.evaluateOnNewDocument((tenantId) => {
    const user = {
      id: 'mobile-planner-e2e-user',
      tenantId,
      email: 'admin@johnnyone.local',
      displayName: 'Mobile Planner E2E',
      roles: ['admin'],
      status: 'active',
    };
    window.localStorage.setItem('johnnyone_worker_url', 'http://127.0.0.1:7714');
    window.localStorage.setItem('johnnyone_access_token', 'mobile-planner-e2e-token');
    window.localStorage.setItem('johnnyone_refresh_token', 'mobile-planner-e2e-refresh-token');
    window.localStorage.setItem('johnnyone_tenant_id', tenantId);
    window.localStorage.setItem('johnnyone_user_id', user.id);
    window.localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));
    window.__johnnyonePlannerInputs = [];

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
          if (data.data) window.__johnnyonePlannerInputs.push({ sessionId: data.sessionId, data: data.data });
          if (data.sessionId && (data.control || data.data)) {
            this.emitScreen(data.sessionId, data.data || '');
          }
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
        this.onclose?.({ type: 'close' });
      }

      addEventListener(type, handler) {
        this[`on${type}`] = handler;
      }

      removeEventListener(type) {
        this[`on${type}`] = null;
      }

      emitScreen(sessionId, input) {
        const role = sessionId.includes('reviewer') ? 'T2 reviewer' : 'T1 worker';
        const prior = `JohnnyOne ${role} mock terminal\n${sessionId}\n>>> `;
        const content = input ? `${prior}${input.replace(/\r/g, '\n')}\n>>> ` : prior;
        this.onmessage?.({
          data: JSON.stringify({
            type: 'terminal_screen',
            data: {
              sessionId,
              status: 'attached',
              cursor: Date.now(),
              content,
              cols: 80,
              rows: 24,
              tmuxSessionName: `tmux-${sessionId}`,
              paneId: '%1',
            },
          }),
        });
      }
    }

    window.WebSocket = MockRelayWebSocket;
  }, TENANT_ID);
}

async function mockGraphql(page) {
  const runs = {
    planning: createRun('planning'),
    development: createRun('development'),
  };

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
    const variables = parsed.variables || {};
    const query = parsed.query || body;
    const runType = variables.runType === 'planning' ? 'planning' : variables.runType === 'development' ? 'development' : null;
    const byId = variables.id === 'planning-plan' ? runs.planning : variables.id === 'development-plan' ? runs.development : null;
    let data = {};

    if (query.includes('listAgentPlans')) {
      const selectedRun = runType ? runs[runType] : runs.development;
      data = { listAgentPlans: [selectedRun.plan] };
    } else if (query.includes('getAgentPlan')) {
      data = { getAgentPlan: byId || runs.development };
    } else if (query.includes('startAgentPlan')) {
      data = { startAgentPlan: byId || runs.development };
    } else if (query.includes('sendAgentFeedbackToWorker')) {
      data = { sendAgentFeedbackToWorker: byId || runs.development };
    } else if (query.includes('retryAgentReviewer')) {
      data = { retryAgentReviewer: byId || runs.development };
    } else if (query.includes('updateAgentPhaseManualPass')) {
      data = { updateAgentPhaseManualPass: byId || runs.development };
    } else if (query.includes('updateAgentPlanStopped')) {
      data = { updateAgentPlanStopped: byId || runs.development };
    } else if (query.includes('updateAgentPlanBlocked')) {
      data = { updateAgentPlanBlocked: byId || runs.development };
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
    } else if (query.includes('listWorkspaceFiles')) {
      data = {
        listWorkspaceFiles: [
          { path: 'desktop/src/app/pages/planner/planner.page.html', name: 'planner.page.html', kind: 'file', status: 'M', size: 12000 },
          { path: 'desktop/e2e/mobile-planner-panels.e2e.cjs', name: 'mobile-planner-panels.e2e.cjs', kind: 'file', status: 'A', size: 8000 },
        ],
      };
    } else if (query.includes('getWorkspaceFileDiff')) {
      data = { getWorkspaceFileDiff: { path: variables.path, diff: '@@ mobile planner @@\n+ panel switcher\n+ mobile files modal\n' } };
    } else if (query.includes('browseHostDirectory')) {
      data = {
        browseHostDirectory: [
          { path: `${variables.path || '/workspace'}/overview.md`, name: 'overview.md', kind: 'file', size: 2100 },
          { path: `${variables.path || '/workspace'}/phases`, name: 'phases', kind: 'directory', size: 0 },
        ],
      };
    } else if (query.includes('readHostFile')) {
      data = {
        readHostFile: {
          path: variables.path,
          name: String(variables.path || 'overview.md').split('/').pop(),
          kind: 'file',
          contentType: String(variables.path || '').endsWith('.yml') ? 'text/yaml' : 'text/markdown',
          encoding: 'utf8',
          content: String(variables.path || '').endsWith('.yml')
            ? 'schema: task-status/v1\nstate: in-progress\nowner: T1_WORKER\nvalidation:\n  summary: Mobile mock status\n'
            : '# Mobile Mock Plan\n\nThis file is rendered in the mobile files preview.',
          size: 160,
        },
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

async function waitForPlannerReady(page, route) {
  await page.goto(`${APP_URL.replace(/\/$/, '')}${route}`, { waitUntil: 'networkidle2', timeout: 60000 });
  await page.waitForSelector('.planner-shell', { timeout: 30000 });
  await page.waitForSelector('.mobile-panel-switcher', { timeout: 30000 });
  await page.waitForSelector('.terminal-card.active-mobile-panel johnny-terminal-screen .xterm-rows', { timeout: 30000 });
}

async function assertNoOverflow(page, label) {
  const metrics = await page.evaluate(() => {
    const active = document.querySelector('.terminal-card.active-mobile-panel, .coordinator-panel.active-mobile-panel');
    const shell = document.querySelector('.planner-shell');
    const modal = document.querySelector('.files-modal, .phase-tasks-modal');
    const rect = (element) => {
      if (!element) return null;
      const box = element.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: box.width, height: box.height };
    };
    return {
      viewportWidth: window.innerWidth,
      documentWidth: document.documentElement.scrollWidth,
      active: rect(active),
      shell: rect(shell),
      modal: rect(modal),
      activePanels: document.querySelectorAll('.terminal-card.active-mobile-panel, .coordinator-panel.active-mobile-panel').length,
    };
  });
  assert(metrics.documentWidth <= metrics.viewportWidth + 2, `${label} has horizontal overflow: ${JSON.stringify(metrics)}`);
  assert(metrics.activePanels === 1 || metrics.modal, `${label} should show one active mobile panel: ${JSON.stringify(metrics)}`);
  assert(!metrics.active || metrics.active.right <= metrics.viewportWidth + 2, `${label} active panel clipped: ${JSON.stringify(metrics)}`);
  assert(!metrics.modal || metrics.modal.right <= metrics.viewportWidth + 2, `${label} modal clipped: ${JSON.stringify(metrics)}`);
}

async function switchPanel(page, text) {
  await page.evaluate((label) => {
    const buttons = Array.from(document.querySelectorAll('.mobile-panel-switcher button'));
    buttons.find((button) => (button.textContent || '').trim() === label)?.click();
  }, text);
  await delay(250);
}

async function submitComposer(page, text) {
  const selector = '.terminal-card.active-mobile-panel johnny-terminal-screen textarea[name="terminalInput"]';
  await page.waitForSelector(selector, { timeout: 15000 });
  await page.focus(selector);
  await page.keyboard.type(text);
  const value = await page.$eval(selector, (element) => element.value);
  assert(value.includes(text), `composer did not retain typed text: ${value}`);
  await page.keyboard.press('Enter');
  await page.waitForFunction((needle) => window.__johnnyonePlannerInputs.some((item) => item.data.includes(needle)), { timeout: 5000 }, text);
}

async function runSurface(page, config) {
  await waitForPlannerReady(page, config.route);
  await assertNoOverflow(page, `${config.name} T1`);
  await screenshot(page, config.taskId, config.t1Shot);
  await submitComposer(page, `${config.name} T1 mobile message`);

  await switchPanel(page, 'T2');
  await page.waitForSelector('.terminal-card.active-mobile-panel johnny-terminal-screen textarea[name="terminalInput"]', { timeout: 15000 });
  await assertNoOverflow(page, `${config.name} T2`);
  await screenshot(page, config.taskId, config.t2Shot);
  await submitComposer(page, `${config.name} T2 mobile message`);

  await switchPanel(page, 'Coordinator');
  await page.waitForSelector('.coordinator-panel.active-mobile-panel', { timeout: 15000 });
  await assertNoOverflow(page, `${config.name} Coordinator`);
  await screenshot(page, config.taskId, config.coordinatorShot);

  if (config.phaseShot) {
    await page.evaluate(() => {
      document.querySelector('.coordinator-panel.active-mobile-panel .phase-row')?.click();
    });
    await page.waitForSelector('.phase-tasks-modal', { timeout: 15000 });
    await assertNoOverflow(page, `${config.name} phase status`);
    await screenshot(page, config.taskId, config.phaseShot);
    await page.click('.phase-tasks-modal .icon-action');
  }

  await page.evaluate(() => {
    const buttons = Array.from(document.querySelectorAll('.coordinator-panel.active-mobile-panel .coordinator-actions button'));
    buttons.find((button) => (button.textContent || '').trim() === 'Files')?.click();
  });
  await page.waitForSelector('.files-modal', { timeout: 15000 });
  await assertNoOverflow(page, `${config.name} files`);
  await screenshot(page, config.taskId, config.filesShot);
  await page.click('.files-tabs button:nth-child(2)');
  await delay(250);
  await assertNoOverflow(page, `${config.name} plan files`);
  await page.click('.files-modal .icon-action');
}

async function run() {
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

    await runSurface(page, {
      route: '/planning',
      name: 'planning',
      taskId: '01-planning-mobile-panels',
      t1Shot: '02-01-390x844-planning-t1-panel',
      t2Shot: '02-01-390x844-planning-t2-panel',
      coordinatorShot: '02-01-390x844-planning-coordinator',
      filesShot: '02-01-390x844-planning-files-modal',
    });

    await page.setViewport({ width: 430, height: 932, isMobile: true, hasTouch: true });
    await runSurface(page, {
      route: '/development',
      name: 'development',
      taskId: '02-development-mobile-panels',
      t1Shot: '02-02-430x932-development-t1-panel',
      t2Shot: '02-02-430x932-development-t2-panel',
      coordinatorShot: '02-02-430x932-development-coordinator',
      phaseShot: '02-02-430x932-development-phase-status',
      filesShot: '02-02-430x932-development-files-modal',
    });

    console.log('mobile-planner-panels.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
