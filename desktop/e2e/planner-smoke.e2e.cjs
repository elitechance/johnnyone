const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'http://127.0.0.1:4200';
const WORKER_URL = process.env.JOHNNYONE_E2E_WORKER_URL || 'http://127.0.0.1:7714';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR
  || path.resolve(__dirname, 'artifacts/planner-smoke');
const TENANT_ID = '00000000-0000-0000-0000-000000000001';
let accessToken = null;

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function postGraphql(query, variables) {
  const response = await fetch(`${WORKER_URL.replace(/\/$/, '')}/graphql`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      'x-tenant-id': TENANT_ID,
      ...(accessToken ? { Authorization: `Bearer ${accessToken}` } : {}),
    },
    body: JSON.stringify({ query, variables }),
  });
  const payload = await response.json();
  if (payload.errors?.length) {
    throw new Error(payload.errors.map((error) => error.message).join('; '));
  }
  return payload.data;
}

async function login() {
  const data = await postGraphql(
    `mutation Login($input: LoginInput!) {
      login(input: $input) {
        accessToken
        refreshToken
        user { id tenantId email displayName roles status }
      }
    }`,
    {
      input: {
        email: 'admin@johnnyone.local',
        password: 'johnnyone-dev',
        tenantId: TENANT_ID,
      },
    },
  );
  return data.login;
}

async function primeAuth(page, auth) {
  await page.evaluateOnNewDocument((workerUrl, loginResult) => {
    window.localStorage.setItem('johnnyone_worker_url', workerUrl);
    window.localStorage.setItem('johnnyone_access_token', loginResult.accessToken);
    window.localStorage.setItem('johnnyone_refresh_token', loginResult.refreshToken);
    window.localStorage.setItem('johnnyone_tenant_id', loginResult.user.tenantId);
    window.localStorage.setItem('johnnyone_user_id', loginResult.user.id);
    window.localStorage.setItem('johnnyone_auth_user', JSON.stringify(loginResult.user));
  }, WORKER_URL, auth);
}

async function screenshot(page, name) {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const filePath = path.join(ARTIFACT_DIR, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: true });
  console.log(`planner-smoke.e2e: screenshot ${filePath}`);
}

async function run() {
  const auth = await login();
  accessToken = auth.accessToken;
  const testTitle = `E2E planner smoke ${Date.now()}`;
  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1440, height: 920 });
    await primeAuth(page, auth);
    page.on('pageerror', (error) => console.error(`[pageerror] ${error.message}`));

    await page.goto(`${APP_URL.replace(/\/$/, '')}/planner`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.planner-shell', { timeout: 30000 });
    await screenshot(page, '01-planner-view');

    await page.click('.planner-actions .primary-action');
    await page.waitForSelector('.setup-modal', { timeout: 15000 });
    await page.waitForFunction(
      () => document.querySelector('.validation-box')?.textContent?.includes('Valid plan'),
      { timeout: 30000 },
    );
    await screenshot(page, '02-new-planner-modal');

    const validationIsDynamic = await page.evaluate(() => {
      const text = document.querySelector('.validation-box')?.textContent || '';
      return text.includes('Agent Validation Loop Plan') && text.includes('4 phases') && text.includes('10 tasks');
    });
    assert(validationIsDynamic, 'Plan validation should show backend parsed title, phase count, and task count');

    await page.evaluate(() => {
      const buttons = Array.from(document.querySelectorAll('.field-control button'));
      buttons.find((button) => button.textContent?.includes('Browse'))?.click();
    });
    await page.waitForSelector('.browser-modal', { timeout: 15000 });
    await screenshot(page, '03-path-browser');
    const browserHasHostPath = await page.evaluate(() => {
      const modal = document.querySelector('.browser-modal');
      return Boolean(modal?.textContent?.includes('/home/creepy/documents/workspace'));
    });
    assert(browserHasHostPath, 'Path browser should browse backend host directories');
    await page.click('.browser-modal .icon-action');

    const hasOnlyPlannerAction = await page.evaluate(() => {
      const actions = Array.from(document.querySelectorAll('.planner-actions button')).map((button) => button.textContent || '');
      return actions.some((text) => text.includes('New Planner')) && !actions.some((text) => text.includes('New Terminal'));
    });
    assert(hasOnlyPlannerAction, 'Planner view should show New Planner and not New Terminal');

    const terminalTabVisible = await page.evaluate(() => document.querySelector('.workspace-tab') !== null);
    assert(!terminalTabVisible, 'Planner view should not show terminal session tabs');

    const created = await postGraphql(
      `mutation CreateAgentPlan($input: CreateAgentPlanInput!) {
        createAgentPlan(input: $input) {
          plan { id title }
        }
      }`,
      {
        input: {
          title: testTitle,
          workspacePath: '/home/creepy/documents/workspace',
          planPath: 'personal/docs/johnnyone/plans/agent-validation-loop',
          workerProvider: 'ollama',
          reviewerProvider: 'ollama',
        },
      },
    );
    const planId = created.createAgentPlan.plan.id;

    await page.goto(`${APP_URL.replace(/\/$/, '')}/planner`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForFunction((title) => document.body.textContent.includes(title), { timeout: 30000 }, testTitle);
    await page.evaluate((title) => {
      const tabs = Array.from(document.querySelectorAll('.planner-tab'));
      const tab = tabs.find((candidate) => candidate.textContent.includes(title));
      tab?.querySelector('.planner-tab-main')?.click();
    }, testTitle);
    await page.waitForFunction(
      (title) => document.querySelector('.coordinator-panel')?.textContent?.includes(title),
      { timeout: 30000 },
      testTitle,
    );
    await page.click('.coordinator-actions .secondary-action');
    await page.waitForSelector('.files-modal', { timeout: 15000 });
    await screenshot(page, '04-files-modal');

    const hasGlobalError = await page.evaluate(() => document.querySelector('.error-banner') !== null);
    assert(!hasGlobalError, 'Files modal should not create a global error banner');

    const filesModalHasContent = await page.evaluate(() => {
      const modal = document.querySelector('.files-modal');
      return Boolean(modal && modal.textContent && modal.textContent.trim().length > 0);
    });
    assert(filesModalHasContent, 'Files modal should show an empty, info, or file state');

    await page.click('.files-modal .icon-action');
    await page.click('.planner-tab.active .tab-close');
    await page.waitForFunction((title) => !document.body.textContent.includes(title), { timeout: 30000 }, testTitle);
    await postGraphql(
      `mutation DeleteAgentPlan($id: ID!) {
        deleteAgentPlan(id: $id)
      }`,
      { id: planId },
    ).catch(() => undefined);

    console.log('planner-smoke.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
