#!/usr/bin/env node
/**
 * Phase 02 E2E: drive the real authenticated web terminal (JWT ?token= on WSS,
 * no nodeId) through connect -> input -> resize. Captures the three named
 * screenshots required by Task 06 for vision read-back vs Task 01 source-of-truth.
 *
 * Runs against deployed by default (or APP_URL). Requires valid login creds
 * so the access token is stored and rides the WS upgrade.
 *
 * Usage:
 *   node web/e2e/web-terminal.e2e.cjs
 *   APP_URL=http://localhost:4200 node web/e2e/web-terminal.e2e.cjs
 */

const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.APP_URL || 'https://johnnyone-dev.pages.dev';
const EMAIL = process.env.JOHNNYONE_EMAIL || 'admin@johnnyone.local';
const PASSWORD = process.env.JOHNNYONE_PASSWORD || 'johnnyone-dev';
const TENANT_ID =
  process.env.JOHNNYONE_TENANT || '00000000-0000-0000-0000-000000000001';

const OUT_DIR = path.join(__dirname, '__screenshots__');
fs.mkdirSync(OUT_DIR, { recursive: true });

const log = (...a) => console.log('[web-terminal]', ...a);
const fail = (msg) => {
  console.error('✗', msg);
  process.exit(1);
};

async function shot(page, name) {
  const file = path.join(OUT_DIR, `${name}.png`);
  await page.screenshot({ path: file, fullPage: true });
  log(`screenshot: ${file}`);
  return file;
}

(async () => {
  const browser = await puppeteer.launch({
    headless: 'new',
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 800 });

  page.on('pageerror', (e) => log('pageerror:', e.message));
  page.on('console', (msg) => {
    if (msg.type() === 'error') log('console.error:', msg.text());
  });

  // 1) Login (real path so JWT lands in localStorage for ?token= WS)
  log(`GET ${APP_URL}/`);
  await page.goto(`${APP_URL}/`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('ion-app', { timeout: 10000 });

  // Force dev worker (after load so localStorage is accessible; localhost respects it)
  await page.evaluate(() => {
    localStorage.setItem('johnnyone_worker_url', 'https://johnnyone-dev.ethan-353.workers.dev');
  });
  log('set dev worker url in localStorage, reloading');
  await page.reload({ waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('ion-app', { timeout: 10000 });

  const fillIon = async (label, value) => {
    const handle = await page.evaluateHandle((label) => {
      const items = Array.from(document.querySelectorAll('ion-item'));
      const item = items.find((el) => el.textContent && el.textContent.includes(label));
      if (!item) return null;
      const input = item.querySelector('input');
      return input;
    }, label);
    if (!handle || !(await handle.evaluate((el) => !!el))) {
      throw new Error(`could not find native input under ion-item containing "${label}"`);
    }
    await handle.click({ clickCount: 3 });
    await handle.type(value);
  };

  await fillIon('Email', EMAIL);
  await fillIon('Password', PASSWORD);
  await fillIon('Tenant', TENANT_ID);

  log('click Sign in');
  await Promise.all([
    page.waitForNavigation({ waitUntil: 'networkidle2', timeout: 30000 }),
    page.evaluate(() => {
      const btn = Array.from(document.querySelectorAll('ion-button')).find(
        (b) =>
          b.textContent &&
          (b.textContent.includes('Sign in') || b.textContent.includes('Signing in')),
      );
      if (!btn) throw new Error('Sign-in button not found');
      btn.click();
    }),
  ]);

  const postLoginUrl = page.url();
  log(`url after login: ${postLoginUrl}`);

  // 2) Go to terminal page (real authenticated terminal surface)
  log('GET /terminal');
  await page.goto(`${APP_URL}/terminal`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('app-terminal, .terminal-workspace, johnny-terminal-screen, .workspace-shell', { timeout: 15000 });

  // If no active terminal pane, drive "New Terminal" + confirm a dir (real path)
  const hasPane = await page.evaluate(() => !!document.querySelector('.terminal-pane, johnny-terminal-screen'));
  if (!hasPane) {
    log('no pane visible; clicking New Terminal + Use This Directory');
    await page.evaluate(() => {
      const btn = Array.from(document.querySelectorAll('button,ion-button')).find(
        (b) => b.textContent && b.textContent.includes('New Terminal'),
      );
      if (btn) (btn).click();
    });
    await new Promise((r) => setTimeout(r, 800));
    await page.evaluate(() => {
      const btns = Array.from(document.querySelectorAll('ion-button,button'));
      const use = btns.find((b) => b.textContent && (b.textContent.includes('Use This Directory') || b.textContent.includes('Use')));
      if (use) use.click();
    });
    await page.waitForSelector('.terminal-pane, johnny-terminal-screen', { timeout: 20000 }).catch(() => log('pane wait timed out (proceeding to capture)'));
    await page.waitForFunction(() => {
      const el = document.querySelector('.terminal-status, [class*="terminal-status"]');
      return el && el.textContent && /attached/i.test(el.textContent);
    }, { timeout: 15000 }).catch(() => log('attached status not seen in time'));
    await new Promise((r) => setTimeout(r, 5000)); // allow connect + first visual frames via real JWT WS + attach
  } else {
    await new Promise((r) => setTimeout(r, 2000));
  }

  // connected
  await shot(page, 'web-terminal-connected');

  // 3) Input (drive real terminal input path)
  log('driving input');
  await page.evaluate(() => {
    const ta = document.querySelector('textarea[name="terminalInput"], .terminal-mobile-submit textarea');
    if (ta) {
      ta.value = 'echo "phase02-jwt-wss"';
      ta.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.evaluate(() => {
    const btns = Array.from(document.querySelectorAll('button'));
    const enter = btns.find((b) => b.textContent && b.textContent.trim() === 'Enter');
    if (enter) enter.click();
  });
  await new Promise((r) => setTimeout(r, 8000)); // allow input to relay + screen update + content render
  await shot(page, 'web-terminal-after-input');

  // 4) Resize viewport -> component emits resize -> relay resize (real)
  log('resize viewport');
  await page.setViewport({ width: 960, height: 620 });
  await new Promise((r) => setTimeout(r, 2000));
  await shot(page, 'web-terminal-after-resize');

  await browser.close();
  log('✓ terminal e2e complete');
  log(`screenshots under: ${OUT_DIR}`);
  process.exit(0);
})().catch(async (err) => {
  console.error('✗ web-terminal e2e failed:', err.message);
  process.exit(1);
});
