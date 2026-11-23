#!/usr/bin/env node
/**
 * Plans tab — LIVE end-to-end smoke (initiative 8ad3ae71, phase 03 task 10). Human-run (D10): the
 * review lenses are vision:false and do not run it. Needs the deployed worker (plans resolvers), the
 * desktop engine with phase 02 (`npm run desktop`) and a logged-in tenant.
 *
 * Flow: login → /plans → New (title + workspace) → send a chat turn → wait for a `brief.update` tool
 * row and a v2 chip → undo → Start planning (confirm if asked) → lands on /initiatives.
 * Screenshots for the human read-back: plans-list.png, plan-detail.png, plan-phone.png (400×860).
 *
 * Usage:
 *   APP_URL=https://johnnyone.pages.dev WORKSPACE=/abs/path/to/repo node web/e2e/plans-smoke.e2e.cjs
 */
const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.APP_URL || 'https://johnnyone.pages.dev';
const EMAIL = process.env.JOHNNYONE_EMAIL || 'admin@johnnyone.local';
const PASSWORD = process.env.JOHNNYONE_PASSWORD || 'johnnyone-dev';
const TENANT_ID = process.env.JOHNNYONE_TENANT || '00000000-0000-0000-0000-000000000001';
const WORKSPACE_LEAF = process.env.WORKSPACE_LEAF || 'johnnyone';
const AGENT_TIMEOUT_MS = Number(process.env.AGENT_TIMEOUT_MS || 240_000);
const OUT_DIR = process.env.OUT_DIR || path.join(__dirname, '__screenshots__', 'plans-smoke');
fs.mkdirSync(OUT_DIR, { recursive: true });

const log = (...a) => console.log('[plans-smoke]', ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const fail = (msg) => {
  console.error('✗', msg);
  process.exit(1);
};

async function shot(page, name) {
  await sleep(800);
  const file = path.join(OUT_DIR, `${name}.png`);
  await page.screenshot({ path: file });
  log('screenshot:', file);
}

/** Click the first VISIBLE element matching `selector` whose text contains `text`. */
async function clickText(page, selector, text) {
  const ok = await page.evaluate(
    (sel, t) => {
      const vis = (el) => !!(el.offsetWidth || el.offsetHeight) && !el.closest('.ion-page-hidden');
      const el = Array.from(document.querySelectorAll(sel)).find((e) => vis(e) && (e.textContent || '').includes(t));
      if (el) el.click();
      return !!el;
    },
    selector,
    text,
  );
  if (!ok) throw new Error(`no visible ${selector} containing "${text}"`);
}

(async () => {
  const browser = await puppeteer.launch({ headless: 'new', args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  const page = await browser.newPage();
  await page.setUserAgent((await browser.userAgent()).replace('HeadlessChrome', 'Chrome'));
  await page.setViewport({ width: 1280, height: 860 });
  page.on('pageerror', (e) => log('pageerror:', e.message));

  // 1) login
  await page.goto(`${APP_URL}/login`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('ion-button', { timeout: 20000 });
  for (const [label, value] of [['Email', EMAIL], ['Password', PASSWORD], ['Tenant', TENANT_ID]]) {
    const h = await page.evaluateHandle((l) => {
      const item = Array.from(document.querySelectorAll('ion-item')).find((el) => (el.textContent || '').includes(l));
      return item ? item.querySelector('input') : null;
    }, label);
    if (!(await h.evaluate((el) => !!el))) fail(`login field ${label} not found`);
    await h.click({ clickCount: 3 });
    await h.type(value);
  }
  await clickText(page, 'ion-button', 'Sign in');
  await page.waitForFunction(() => !location.pathname.startsWith('/login'), { timeout: 30000 });

  // 2) /plans
  await page.goto(`${APP_URL}/plans`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForFunction(() => /No draft plans yet|plan-card|Couldn't load plans/.test(document.body.innerHTML), { timeout: 30000 });
  if (await page.evaluate(() => document.body.innerText.includes("Couldn't load plans"))) fail('the plans list failed to load (worker/engine not deployed?)');
  await shot(page, 'plans-list');

  // 3) New plan: title + workspace via the picker
  await clickText(page, 'ion-button', 'New plan');
  await sleep(800);
  const title = await page.$('ion-modal ion-input input');
  if (!title) fail('New plan modal has no title input');
  await title.type(`Smoke plan ${new Date().toISOString().slice(0, 16)}`);
  await clickText(page, 'ion-item', 'Workspace');
  await sleep(1500);
  // Walk down to the workspace leaf if it is listed, then use the current folder.
  await clickText(page, 'ion-item', WORKSPACE_LEAF).catch(() => log(`"${WORKSPACE_LEAF}" not listed at the browse root — using the root`));
  await sleep(1200);
  await clickText(page, 'ion-button', 'Use this folder');
  await sleep(600);
  await clickText(page, 'ion-button', 'Create');
  await page.waitForFunction(() => /^\/plans\/[^/]+$/.test(location.pathname), { timeout: 30000 });
  const planUrl = page.url();
  log('created:', planUrl);

  // 4) one chat turn → the agent edits the brief through MCP
  await page.waitForSelector('johnny-chat-window textarea', { timeout: 30000 });
  const ta = await page.$('johnny-chat-window textarea');
  await ta.type('Draft the outcome in one sentence: a smoke test for the Plans tab.');
  await ta.press('Enter');
  log('waiting for a brief.update tool row and a v2 chip…');
  await page.waitForFunction(
    () => {
      const rows = Array.from(document.querySelectorAll('.tool-row-name')).map((e) => e.textContent || '');
      const chips = Array.from(document.querySelectorAll('app-version-chip')).map((e) => e.textContent || '');
      return rows.some((t) => t.includes('brief.update')) && chips.some((t) => t.includes('v2'));
    },
    { timeout: AGENT_TIMEOUT_MS },
  );
  await shot(page, 'plan-detail');

  // 5) undo the agent's change → a new version authored by you
  const before = await page.evaluate(() => document.querySelectorAll('app-version-chip').length);
  await clickText(page, 'ion-button', 'undo');
  await page.waitForFunction((n) => document.querySelectorAll('app-version-chip').length > n, { timeout: 30000 }, before);

  // 6) phone layout
  await page.setViewport({ width: 400, height: 860, isMobile: true, hasTouch: true });
  await page.goto(planUrl, { waitUntil: 'networkidle2', timeout: 30000 });
  await sleep(1500);
  await shot(page, 'plan-phone');
  await page.setViewport({ width: 1280, height: 860 });
  await page.goto(planUrl, { waitUntil: 'networkidle2', timeout: 30000 });
  await sleep(1500);

  // 7) Start planning (confirm "Start anyway" if the check has failures) → /initiatives
  await clickText(page, 'ion-button', 'Start planning');
  await sleep(800);
  await clickText(page, 'button', 'Start anyway').catch(() => undefined);
  await page.waitForFunction(() => location.pathname.startsWith('/initiatives'), { timeout: 60000 });
  log('✓ landed on', page.url());
  await browser.close();
  process.exit(0);
})().catch((err) => fail(err.message));
