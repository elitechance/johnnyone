const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'http://127.0.0.1:4200';
const WORKER_URL = process.env.JOHNNYONE_E2E_WORKER_URL || 'http://127.0.0.1:7714';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR
  || path.resolve(__dirname, 'artifacts/mobile-shell');
const TENANT_ID = process.env.JOHNNYONE_E2E_TENANT_ID || '00000000-0000-0000-0000-000000000001';

const VIEWPORTS = [
  { label: '390x844', width: 390, height: 844 },
  { label: '430x932', width: 430, height: 932 },
];

const SURFACES = [
  {
    route: '/terminal',
    shell: '.workspace-shell',
    topbar: '.workspace-topbar',
    primary: 'New Terminal',
    surface: 'terminal',
  },
  {
    route: '/planning',
    shell: '.planner-shell',
    topbar: '.planner-topbar',
    primary: 'New Planner',
    surface: 'planning',
  },
  {
    route: '/development',
    shell: '.planner-shell',
    topbar: '.planner-topbar',
    primary: 'New Development',
    surface: 'development',
  },
];

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function primeAuth(page) {
  await page.evaluateOnNewDocument((workerUrl, tenantId) => {
    try {
      const user = {
        id: 'mobile-shell-e2e-user',
        tenantId,
        email: 'admin@johnnyone.local',
        displayName: 'Mobile Shell E2E',
        roles: ['admin'],
        status: 'active',
      };
      window.localStorage.setItem('johnnyone_worker_url', workerUrl);
      window.localStorage.setItem('johnnyone_access_token', 'mobile-shell-e2e-token');
      window.localStorage.setItem('johnnyone_refresh_token', 'mobile-shell-e2e-refresh-token');
      window.localStorage.setItem('johnnyone_tenant_id', tenantId);
      window.localStorage.setItem('johnnyone_user_id', user.id);
      window.localStorage.setItem('johnnyone_auth_user', JSON.stringify(user));
    } catch {
      // Some browser bootstrap documents do not expose localStorage.
    }
  }, WORKER_URL, TENANT_ID);
}

async function mockGraphql(page) {
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

    if (request.method() !== 'POST') {
      request.continue();
      return;
    }

    const body = request.postData() || '{}';
    let operationName = '';
    try {
      operationName = JSON.parse(body).operationName || '';
    } catch {
      // Fall through to query text checks below.
    }

    const data = graphqlMockData(operationName, body);
    request.respond({
      status: 200,
      contentType: 'application/json',
      headers: {
        'Access-Control-Allow-Origin': '*',
      },
      body: JSON.stringify({ data }),
    });
  });
}

function graphqlMockData(operationName, body) {
  if (operationName === 'ListSessions' || body.includes('listAiSessions')) {
    return { listAiSessions: [] };
  }
  if (operationName === 'ListDetectedCliTools' || body.includes('listDetectedCliTools')) {
    return {
      listDetectedCliTools: [
        { provider: 'ollama', command: 'ollama', found: true, path: '/usr/bin/ollama' },
      ],
    };
  }
  if (operationName === 'GetSetting' || body.includes('getSetting')) {
    return { getSetting: '/home/creepy/documents/workspace' };
  }
  if (operationName === 'ListAgentPlans' || body.includes('listAgentPlans')) {
    return { listAgentPlans: [] };
  }
  if (operationName === 'GetPlannerPromptSettings' || body.includes('getPlannerPromptSettings')) {
    return {
      getPlannerPromptSettings: {
        schema: 'planner-prompt-settings/v1',
        development: { worker: '', reviewer: '' },
        planning: { planner: '', reviewer: '' },
      },
    };
  }
  if (operationName === 'ValidateWorkspacePlan' || body.includes('validateWorkspacePlan')) {
    return {
      validateWorkspacePlan: {
        valid: true,
        workspacePath: '/home/creepy/documents/workspace',
        planPath: 'personal/docs/johnnyone/plans/agent-validation-loop',
        title: 'Mock Plan',
        phaseCount: 1,
        taskCount: 1,
        error: null,
      },
    };
  }
  return {};
}

async function screenshot(page, name) {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const filePath = path.join(ARTIFACT_DIR, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: true });
  console.log(`mobile-shell.e2e: screenshot ${filePath}`);
}

async function assertMobileShell(page, surface) {
  const geometry = await page.evaluate((config) => {
    const topbar = document.querySelector(config.topbar);
    const shell = document.querySelector(config.shell);
    const modeSwitch = document.querySelector('.mode-switch');
    const primaryButtons = Array.from(document.querySelectorAll('button'))
      .filter((button) => (button.textContent || '').includes(config.primary));
    const primary = primaryButtons[0] || null;
    const actions = document.querySelector('.workspace-actions, .planner-actions');
    const tabs = document.querySelector('.workspace-tabs, .planner-tabs');
    const rect = (element) => {
      if (!element) return null;
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
      shell: rect(shell),
      topbar: rect(topbar),
      modeSwitch: rect(modeSwitch),
      primary: rect(primary),
      actions: rect(actions),
      tabs: rect(tabs),
      tabCount: tabs?.children.length || 0,
      primaryText: primary?.textContent || '',
    };
  }, surface);

  assert(geometry.shell, `${surface.surface} shell missing`);
  assert(geometry.topbar, `${surface.surface} topbar missing`);
  assert(geometry.modeSwitch, `${surface.surface} mode switch missing`);
  assert(geometry.primary, `${surface.surface} primary action missing`);
  assert(geometry.primaryText.includes(surface.primary), `${surface.surface} primary action text mismatch`);
  assert(geometry.documentWidth <= geometry.viewportWidth + 2, `${surface.surface} has horizontal page overflow: ${JSON.stringify(geometry)}`);
  assert(geometry.topbar.right <= geometry.viewportWidth + 2, `${surface.surface} topbar is clipped: ${JSON.stringify(geometry)}`);
  assert(geometry.modeSwitch.right <= geometry.viewportWidth + 2, `${surface.surface} mode switch is clipped: ${JSON.stringify(geometry)}`);
  assert(geometry.primary.right <= geometry.viewportWidth + 2, `${surface.surface} primary action is clipped: ${JSON.stringify(geometry)}`);
  assert(geometry.actions?.height >= 32, `${surface.surface} actions row collapsed: ${JSON.stringify(geometry)}`);
  assert(geometry.tabCount === 0 || geometry.tabs.height >= 30, `${surface.surface} tabs row collapsed: ${JSON.stringify(geometry)}`);
}

async function run() {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    const page = await browser.newPage();
    await primeAuth(page);
    await mockGraphql(page);
    page.on('pageerror', (error) => console.error(`[pageerror] ${error.message}`));

    for (const viewport of VIEWPORTS) {
      await page.setViewport({ width: viewport.width, height: viewport.height, isMobile: true, hasTouch: true });

      for (const surface of SURFACES) {
        await page.goto(`${APP_URL.replace(/\/$/, '')}${surface.route}`, { waitUntil: 'networkidle2', timeout: 60000 });
        await page.waitForSelector(surface.shell, { timeout: 30000 });
        await assertMobileShell(page, surface);
        await screenshot(page, `00-01-${viewport.label}-${surface.surface}-header-shell`);
      }
    }

    console.log('mobile-shell.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
