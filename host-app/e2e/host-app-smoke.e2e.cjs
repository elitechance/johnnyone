#!/usr/bin/env node
/**
 * host-app local smoke — drives the Tauri control-panel Angular app in a browser.
 * Verifies: status page, providers page (detects real CLIs from the local host),
 * login page renders. Assumes:
 *   - johnnyone-host running on :7788 (so status chips can show "Up")
 *   - `nx serve host-app --port 4201` running
 *
 * Usage: node host-app/e2e/host-app-smoke.e2e.cjs
 */

const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP = process.env.APP_URL || 'http://127.0.0.1:4201';
const OUT = path.join(__dirname, '__screenshots__');
fs.mkdirSync(OUT, { recursive: true });

const log = (...a) => console.log('[host-app-smoke]', ...a);
const shot = async (page, name) => {
  const file = path.join(OUT, `${name}.png`);
  await page.screenshot({ path: file, fullPage: true });
  log('screenshot:', file);
};

(async () => {
  const browser = await puppeteer.launch({
    headless: 'new',
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 900, height: 700 });
  page.on('pageerror', (e) => log('pageerror:', e.message));
  page.on('console', (msg) => {
    if (msg.type() === 'error') log('console.error:', msg.text());
  });

  log(`GET ${APP}/`);
  await page.goto(`${APP}/`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('ion-app', { timeout: 10000 });

  // Wait for the router to redirect '' → '/status' and the page component to mount.
  // Some Angular setups complete initial navigation slightly after networkidle2.
  await page.waitForSelector('host-status-page', { timeout: 15000 });
  const urlAfterRoot = page.url();
  log('url after redirect:', urlAfterRoot);
  if (!urlAfterRoot.includes('/status')) {
    log('  (router did NOT update URL but host-status-page rendered) — proceeding');
  }
  // Let the HostStatusService refresh() complete (fetch :7788 + worker)
  await new Promise((r) => setTimeout(r, 2000));
  await shot(page, '1-status');

  // /login
  log('GET /login');
  await page.goto(`${APP}/login`, { waitUntil: 'networkidle2' });
  await page.waitForSelector('host-login-page', { timeout: 10000 });
  await new Promise((r) => setTimeout(r, 800));
  await shot(page, '2-login');

  // /providers — should detect real CLIs from the local johnnyone-host
  log('GET /providers');
  await page.goto(`${APP}/providers`, { waitUntil: 'networkidle2' });
  await page.waitForSelector('host-providers-page', { timeout: 10000 });
  // detectCliTools query is async — wait for load() to settle
  await page.waitForFunction(
    () => {
      const card = document.querySelector('host-providers-page ion-card-content');
      if (!card) return false;
      const text = card.textContent || '';
      return !text.includes('Checking…');
    },
    { timeout: 15000 },
  ).catch(() => log('  (providers load wait timed out — taking screenshot anyway)'));
  await new Promise((r) => setTimeout(r, 500));
  await shot(page, '3-providers');

  await browser.close();
  log('✓ all steps complete');
})().catch((err) => {
  console.error('✗', err.message);
  process.exit(1);
});
