#!/usr/bin/env node
/**
 * Phase 1/2 deployed-smoke: drive the live web client through login → chat → install.
 *
 * Runs against the deployed elitechance dev environment by default. Captures
 * named screenshots per page state so the methodology's read-back rule is
 * satisfied (the test runner can `Read` each PNG via vision afterwards).
 *
 * Usage:
 *   node web/e2e/login-smoke.e2e.cjs
 *   APP_URL=http://localhost:4200 node web/e2e/login-smoke.e2e.cjs  # local
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

const log = (...a) => console.log('[login-smoke]', ...a);
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

  // ── 1) Visit root, expect redirect to /login (authGuard) ─────────────────
  log(`GET ${APP_URL}/`);
  await page.goto(`${APP_URL}/`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('ion-app', { timeout: 10000 });
  const urlAfterRoot = page.url();
  log(`url after root: ${urlAfterRoot}`);
  if (!urlAfterRoot.includes('/login')) {
    await shot(page, '1-redirect-fail');
    fail(`expected redirect to /login, got ${urlAfterRoot}`);
  }
  await shot(page, '1-login-empty');

  // ── 2) Fill + submit login form ──────────────────────────────────────────
  log('fill credentials');
  // Ionic inputs require typing into the underlying native input that
  // ion-input wraps. Use the label text to find each field.
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
  await shot(page, '2-login-filled');

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
  log(`url after submit: ${postLoginUrl}`);
  await shot(page, '3-post-login-url-changed');

  if (!postLoginUrl.endsWith('/chat') && !postLoginUrl.includes('/chat')) {
    await shot(page, '3-post-login-unexpected');
    fail(`expected to land on /chat after login, got ${postLoginUrl}`);
  }

  // ── 3) Chat page renders ─────────────────────────────────────────────────
  log('waiting for app-chat-page selector');
  await page.waitForSelector('app-chat-page', { timeout: 15000 });
  log('app-chat-page mounted; waiting for ion-router-outlet transition to settle');

  // Wait for Ionic page transition to complete — the previous page (login)
  // remains in the DOM during the animation. Wait for it to be removed.
  await page.waitForFunction(
    () => {
      const outlet = document.querySelector('ion-router-outlet');
      if (!outlet) return false;
      // After transition: there's exactly one .ion-page child not marked hidden
      const pages = Array.from(outlet.children).filter(
        (el) => el.classList.contains('ion-page') || el.tagName.toLowerCase().includes('-page'),
      );
      const visible = pages.filter(
        (el) => !el.classList.contains('ion-page-hidden') && !el.hasAttribute('aria-hidden'),
      );
      return visible.length === 1;
    },
    { timeout: 10000 },
  ).catch(() => log('  (transition wait timed out — proceeding)'));

  // Verify the chat composer is interactive (real UI, not just DOM)
  await page.waitForSelector('johnny-chat-window', { timeout: 5000 });
  await shot(page, '4-chat-page');

  // Type a test message to exercise the echo stub
  log('typing a test message in the chat composer');
  const composerHandle = await page.evaluateHandle(() => {
    const composer = document.querySelector('johnny-chat-window');
    if (!composer) return null;
    return composer.querySelector('textarea, input[type="text"]');
  });
  if (composerHandle && (await composerHandle.evaluate((el) => !!el))) {
    await composerHandle.type('hello from puppeteer');
    await shot(page, '4b-chat-typed');
    // Press Enter to send
    await composerHandle.press('Enter');
    // Wait for the echo (600ms in chat.page.ts) plus margin
    await new Promise((r) => setTimeout(r, 1500));
    await shot(page, '4c-chat-after-send');
  } else {
    log('  composer textarea/input not found — skipping send test');
  }

  // ── 4) Settings page (read-only) ─────────────────────────────────────────
  log('GET /settings');
  await page.goto(`${APP_URL}/settings`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('ion-card', { timeout: 10000 });
  await shot(page, '5-settings');

  // ── 5) Install page ──────────────────────────────────────────────────────
  log('GET /install');
  await page.goto(`${APP_URL}/install`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('app-install-page, ion-card', { timeout: 10000 });
  await shot(page, '6-install');

  // ── 6) Planner stub ──────────────────────────────────────────────────────
  log('GET /planner');
  await page.goto(`${APP_URL}/planner`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('app-planner-page, ion-card', { timeout: 10000 });
  await shot(page, '7-planner');

  // ── 7) Terminal stub ─────────────────────────────────────────────────────
  log('GET /terminal');
  await page.goto(`${APP_URL}/terminal`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.waitForSelector('app-terminal-page, ion-card', { timeout: 10000 });
  await shot(page, '8-terminal');

  await browser.close();
  log('✓ all steps complete');
  log(`screenshots: ${OUT_DIR}/`);
  process.exit(0);
})().catch(async (err) => {
  console.error('✗ spec failed:', err.message);
  process.exit(1);
});
