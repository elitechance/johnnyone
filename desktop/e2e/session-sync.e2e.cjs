const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'https://johnnyone-dev-web.pages.dev';
const WORKER_URL = process.env.JOHNNYONE_E2E_WORKER_URL || 'https://johnnyone-dev-johnnyone-hub.cf-static-5f5.workers.dev';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR
  || path.resolve(__dirname, 'artifacts/session-sync');
const LOGIN_EMAIL = process.env.JOHNNYONE_E2E_EMAIL || 'admin@johnnyone.local';
const LOGIN_PASSWORD = process.env.JOHNNYONE_E2E_PASSWORD || 'johnnyone-dev';
const TENANT_ID = process.env.JOHNNYONE_E2E_TENANT_ID || '00000000-0000-0000-0000-000000000001';

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function postGraphql(query, variables) {
  const response = await fetch(`${WORKER_URL.replace(/\/$/, '')}/graphql`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      'x-tenant-id': TENANT_ID,
    },
    body: JSON.stringify({ query, variables }),
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`GraphQL failed (${response.status}): ${body}`);
  }
  const json = JSON.parse(body);
  if (json.errors?.length) {
    throw new Error(json.errors.map((error) => error.message).join('; '));
  }
  return json.data;
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
        email: LOGIN_EMAIL,
        password: LOGIN_PASSWORD,
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
  console.log(`session-sync.e2e: screenshot ${filePath}`);
}

async function count(page, selector) {
  return page.$$eval(selector, (elements) => elements.length);
}

async function openChat(page, label) {
  page.on('console', (message) => {
    if (message.type() === 'error') {
      console.error(`[${label}:browser:${message.type()}] ${message.text()}`);
    }
  });
  page.on('pageerror', (error) => {
    console.error(`[${label}:pageerror] ${error.message}`);
  });

  await page.goto(`${APP_URL.replace(/\/$/, '')}/chat`, { waitUntil: 'networkidle2', timeout: 60000 });
  await page.waitForSelector('.workspace-shell', { timeout: 30000 });
}

async function waitForAttachedPane(page) {
  await page.waitForSelector('.terminal-pane.active', { timeout: 60000 });
  await page.waitForFunction(
    () => (document.querySelector('.terminal-pane.active .pane-status')?.textContent || '')
      .toLowerCase()
      .includes('attached'),
    { timeout: 60000 },
  );
}

async function run() {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const auth = await login();

  const browser = await puppeteer.launch({
    headless: true,
    protocolTimeout: 240000,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    const desktop = await browser.newPage();
    const mobile = await browser.newPage();
    await desktop.setViewport({ width: 1440, height: 900 });
    await mobile.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true });
    await primeAuth(desktop, auth);
    await primeAuth(mobile, auth);

    await openChat(desktop, 'desktop');
    await openChat(mobile, 'mobile');

    const desktopTabsBefore = await count(desktop, '.workspace-tab');
    const mobileTabsBefore = await count(mobile, '.workspace-tab');
    console.log(`session-sync.e2e: before desktop=${desktopTabsBefore} mobile=${mobileTabsBefore}`);

    console.log('session-sync.e2e: create terminal in desktop client');
    await desktop.click('.workspace-actions .primary-action');
    console.log('session-sync.e2e: wait for desktop attach');
    await waitForAttachedPane(desktop);
    console.log('session-sync.e2e: wait for desktop tab count');
    await desktop.waitForFunction(
      (previousCount) => document.querySelectorAll('.workspace-tab').length > previousCount,
      { timeout: 60000 },
      desktopTabsBefore,
    );
    console.log('session-sync.e2e: wait for mobile tab count');
    await mobile.waitForFunction(
      (previousCount) => document.querySelectorAll('.workspace-tab').length > previousCount,
      { timeout: 60000 },
      mobileTabsBefore,
    );
    await screenshot(desktop, '01-desktop-created-terminal');
    await screenshot(mobile, '02-mobile-saw-terminal');

    const desktopTabsAfterCreate = await count(desktop, '.workspace-tab');
    const mobileTabsAfterCreate = await count(mobile, '.workspace-tab');

    console.log('session-sync.e2e: close terminal in desktop client');
    await desktop.click('.terminal-pane.active .pane-icon');
    console.log('session-sync.e2e: wait for desktop close');
    await desktop.waitForFunction(
      (previousCount) => document.querySelectorAll('.workspace-tab').length === previousCount - 1,
      { timeout: 30000 },
      desktopTabsAfterCreate,
    );
    console.log('session-sync.e2e: wait for mobile close propagation');
    await mobile.waitForFunction(
      (previousCount) => document.querySelectorAll('.workspace-tab').length === previousCount - 1,
      { timeout: 60000 },
      mobileTabsAfterCreate,
    );
    await screenshot(desktop, '03-desktop-closed-terminal');
    await screenshot(mobile, '04-mobile-reflected-close');

    console.log('session-sync.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
