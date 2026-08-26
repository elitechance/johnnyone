#!/usr/bin/env node
/**
 * Canonical Prompt Library capture + live-path walk.
 * One Puppeteer session: login → overlay development.reviewer → PNGs → restore → logs.
 *
 * Usage:
 *   APP_URL=http://127.0.0.1:4280 \
 *   OUT_DIR=<phase artifacts dir> \
 *   node web/e2e/capture-library.mjs
 *
 * S1 populated-desktop uses a 1280×1600 viewport from scrollTop=0 (Ionic
 * ion-content scrolls internally; document fullPage stays 800px).
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer';

const APP_URL = process.env.APP_URL || 'http://127.0.0.1:4280';
const WORKER_URL =
  process.env.JOHNNYONE_WORKER_URL || 'https://johnnyone.ethan-353.workers.dev';
const EMAIL = process.env.JOHNNYONE_EMAIL || 'admin@johnnyone.local';
const PASSWORD = process.env.JOHNNYONE_PASSWORD || 'johnnyone-dev';
const TENANT_ID =
  process.env.JOHNNYONE_TENANT || '00000000-0000-0000-0000-000000000001';
const OUT_DIR = process.env.OUT_DIR
  ? path.resolve(process.env.OUT_DIR)
  : path.dirname(fileURLToPath(import.meta.url));
const SENTINEL = '\n# lib-capture';
const UA =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36';

fs.mkdirSync(OUT_DIR, { recursive: true });
const logLines = [];
const log = (...a) => {
  const line = `[capture] ${new Date().toISOString()} ${a.join(' ')}`;
  console.log(line);
  logLines.push(line);
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function shot(page, name, width, height, { fullPage = false } = {}) {
  await page.evaluate(() => {
    const content = document.querySelector('ion-content');
    const scroll = content?.shadowRoot?.querySelector('.inner-scroll');
    if (scroll) scroll.scrollTop = 0;
    window.scrollTo(0, 0);
    content?.scrollToTop?.();
  }).catch(() => null);
  await page.setViewport({ width, height });
  await sleep(700);
  const file = path.join(OUT_DIR, `${name}.png`);
  // Ionic scrolls inside ion-content, so a tall viewport (not unclip+fullPage)
  // is what actually keeps the header hint and every catalog row in one PNG.
  await page.screenshot({ path: file, fullPage });
  const bytes = fs.statSync(file).size;
  log(`screenshot ${name}.png ${width}x${height} fullPage=${fullPage} bytes=${bytes}`);
  return file;
}

async function fillIon(page, label, value) {
  const handle = await page.evaluateHandle((lbl) => {
    const items = Array.from(document.querySelectorAll('ion-item'));
    const item = items.find((el) => el.textContent && el.textContent.includes(lbl));
    return item ? item.querySelector('input') : null;
  }, label);
  if (!handle || !(await handle.evaluate((el) => !!el))) {
    throw new Error(`native input not found for "${label}"`);
  }
  await handle.click({ clickCount: 3 });
  await handle.type(value);
}

async function clickButton(page, text) {
  const ok = await page.evaluate((t) => {
    const btn = Array.from(document.querySelectorAll('ion-button')).find(
      (b) => b.textContent && b.textContent.includes(t),
    );
    if (!btn) return false;
    const native = btn.shadowRoot?.querySelector('a, button') || btn;
    native.click();
    return true;
  }, text);
  if (!ok) throw new Error(`ion-button "${text}" not found`);
}

async function waitForText(page, text, timeout = 20000) {
  await page.waitForFunction(
    (t) => document.body && document.body.innerText.toLowerCase().includes(t.toLowerCase()),
    { timeout },
    text,
  );
}

async function selectFilter(page, aria, optionText) {
  await page.setViewport({ width: 1280, height: 800 });
  await page.waitForSelector('ion-select', { timeout: 10000 });
  const value =
    optionText === 'All roles' || optionText === 'All scopes'
      ? ''
      : optionText === 'built-in'
        ? 'builtin'
        : optionText;
  const which = aria.toLowerCase().includes('scope') ? 1 : 0;
  const ok = await page.evaluate(
    ({ which, value }) => {
      const sel = document.querySelectorAll('ion-select')[which];
      if (!sel) return false;
      sel.value = value;
      sel.dispatchEvent(new CustomEvent('ionChange', { detail: { value }, bubbles: true }));
      return true;
    },
    { which, value },
  );
  if (!ok) throw new Error(`ion-select index ${which} missing`);
  await sleep(400);
}

async function typeSearch(page, value) {
  await page.waitForSelector('ion-searchbar', { timeout: 10000 });
  await page.evaluate((v) => {
    const bar = document.querySelector('ion-searchbar');
    bar.value = v;
    bar.dispatchEvent(new CustomEvent('ionInput', { detail: { value: v }, bubbles: true }));
    const input = bar.querySelector('input');
    if (input) {
      input.value = v;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
  }, value);
  await sleep(500);
}

async function accordionByKey(page, key) {
  return page.evaluate((k) => {
    const accs = Array.from(document.querySelectorAll('ion-accordion'));
    const acc = accs.find((el) => {
      const val = el.getAttribute('value') || el.value;
      const text = el.textContent || '';
      return val === k || text.includes(k) || (k === 'development.reviewer' && text.includes('Development reviewer'));
    });
    if (!acc) {
      return {
        ok: false,
        badge: '',
        open: false,
        found: accs.map((el) => ({
          value: el.getAttribute('value') || String(el.value || ''),
          head: (el.textContent || '').slice(0, 80),
        })),
      };
    }
    const badge = (acc.querySelector('ion-badge')?.textContent || '').trim();
    return { ok: true, badge, open: true };
  }, key);
}

async function appendReviewer(page) {
  const found = await accordionByKey(page, 'development.reviewer');
  if (!found.ok) throw new Error('development.reviewer accordion missing ' + JSON.stringify(found.found));
  await sleep(400);
  await page.evaluate((sentinel) => {
    const acc = Array.from(document.querySelectorAll('ion-accordion')).find((el) => {
      const val = el.getAttribute('value') || el.value;
      const text = el.textContent || '';
      return val === 'development.reviewer' || text.includes('Development reviewer');
    });
    const ta = acc && acc.querySelector('textarea');
    if (!ta) throw new Error('textarea missing');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(ta, ta.value + sentinel);
    ta.dispatchEvent(new Event('input', { bubbles: true }));
  }, SENTINEL);
  await sleep(300);
  await clickButton(page, 'Save prompts');
  await waitForText(page, 'Saved', 20000);
}

async function resetReviewer(page) {
  const found = await accordionByKey(page, 'development.reviewer');
  if (!found.ok) throw new Error('development.reviewer accordion missing on restore');
  await sleep(500);
  const clicked = await page.evaluate(() => {
    const acc = Array.from(document.querySelectorAll('ion-accordion')).find((el) => {
      const val = el.getAttribute('value') || el.value;
      const text = el.textContent || '';
      return val === 'development.reviewer' || text.includes('Development reviewer');
    });
    if (!acc) return { ok: false, reason: 'accordion missing' };
    const btn = Array.from(acc.querySelectorAll('ion-button')).find((b) =>
      (b.textContent || '').includes('Reset to default'),
    );
    if (!btn) return { ok: false, reason: 'reset button missing' };
    (btn.shadowRoot?.querySelector('a, button') || btn).click();
    return { ok: true };
  });
  log(`reset click ${JSON.stringify(clicked)}`);
  if (!clicked.ok) throw new Error('Reset to default failed: ' + clicked.reason);
  await sleep(400);
  await clickButton(page, 'Save prompts');
  await waitForText(page, 'Saved', 25000);
}

async function badgeFor(page, key) {
  return page.evaluate((k) => {
    const acc = Array.from(document.querySelectorAll('ion-accordion')).find((el) => {
      const val = el.getAttribute('value') || el.value;
      const text = el.textContent || '';
      return val === k || text.includes(k) || (k === 'development.reviewer' && text.includes('Development reviewer'));
    });
    return (acc?.querySelector('ion-badge')?.textContent || '').trim();
  }, key);
}

async function pageInfo(page) {
  return page.evaluate(() => {
    const url = location.href;
    const title = document.querySelector('ion-title')?.textContent?.trim() || '';
    const text = document.body.innerText;
    const rows = Array.from(
      document.querySelectorAll('app-prompt-library-page ion-list ion-item'),
    ).length;
    const keys = Array.from(document.querySelectorAll('app-prompt-library-page ion-item p'))
      .map((el) => (el.textContent || '').trim())
      .filter((t) => t.includes('.'));
    const duplicate = /duplicate to edit/i.test(text);
    const openEd = /open editor/i.test(text);
    const hint = text.includes('Customise a built-in via Duplicate');
    const empty = text.includes('No prompts match.');
    const used = (text.match(/\d+ runs?/g) || []).slice(0, 12);
    const inert = text.includes('not read by current engine');
    const libraryBtn = Array.from(document.querySelectorAll('ion-button')).some((b) =>
      (b.textContent || '').includes('Prompt library'),
    );
    return {
      url,
      title,
      rows,
      keys,
      duplicate,
      openEd,
      hint,
      empty,
      used,
      inert,
      libraryBtn,
      snippet: text.slice(0, 1600),
    };
  });
}

async function listPlanStatuses(page) {
  const token = await page.evaluate(() => localStorage.getItem('johnnyone_access_token'));
  if (!token) return { ok: false, reason: 'no token' };
  const res = await fetch(`${WORKER_URL}/graphql`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${token}`,
      'x-tenant-id': TENANT_ID,
      'user-agent': UA,
    },
    body: JSON.stringify({
      query: '{ listAgentPlans { plan { id status } } }',
    }),
  });
  const json = await res.json();
  const plans = json?.data?.listAgentPlans || [];
  const running = plans.filter((p) => /running/i.test(p?.plan?.status || ''));
  return {
    ok: true,
    total: plans.length,
    running: running.map((p) => `${p.plan.id}:${p.plan.status}`),
  };
}

const live = [];
const note = (s) => {
  live.push(`- ${new Date().toISOString()} ${s}`);
  log(s);
};

(async () => {
  const browser = await puppeteer.launch({
    headless: 'new',
    args: ['--no-sandbox', '--disable-setuid-sandbox', '--disable-dev-shm-usage'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 800 });
  page.setDefaultTimeout(30000);
  page.on('pageerror', (e) => log('pageerror', e.message));

  log(`APP_URL=${APP_URL} WORKER_URL=${WORKER_URL}`);
  await page.goto(`${APP_URL}/login`, { waitUntil: 'networkidle2', timeout: 30000 });
  await page.evaluate((url) => {
    localStorage.setItem('johnnyone_worker_url', url);
  }, WORKER_URL);
  await page.reload({ waitUntil: 'networkidle2' });
  await page.waitForSelector('ion-app', { timeout: 15000 });

  await fillIon(page, 'Email', EMAIL);
  await fillIon(page, 'Password', PASSWORD);
  await fillIon(page, 'Tenant', TENANT_ID);
  await Promise.all([
    page.waitForNavigation({ waitUntil: 'networkidle2', timeout: 30000 }).catch(() => null),
    clickButton(page, 'Sign in'),
  ]);
  await sleep(1500);
  log(`post-login ${page.url()}`);

  const plans = await listPlanStatuses(page);
  log(`listAgentPlans total=${plans.total ?? 0} running=${(plans.running || []).join(',') || 'none'}`);
  note(
    `overlay uses inert development.reviewer regardless of in-flight runs; running=${(plans.running || []).join(',') || 'none'}`,
  );

  await page.goto(`${APP_URL}/settings`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Planner prompts');
  await waitForText(page, 'Development reviewer', 25000);
  await page.waitForSelector('ion-accordion', { timeout: 15000 });
  await shot(page, 'prompt-library-settings-entry', 1280, 800);
  note(`S9 /settings Prompt library visible=${(await pageInfo(page)).libraryBtn} url=${page.url()}`);

  log('overlay development.reviewer');
  await page.goto(`${APP_URL}/settings?prompt=development.reviewer`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Planner prompts');
  await waitForText(page, 'Development reviewer', 25000);
  await page.waitForSelector('ion-accordion', { timeout: 15000 });
  await sleep(600);
  await appendReviewer(page);
  let badge = await badgeFor(page, 'development.reviewer');
  log(`badge after overlay: ${badge}`);
  if (!/custom/i.test(badge)) log('WARN overlay badge not Custom yet');
  note(`overlay development.reviewer sentinel, Settings badge="${badge}"`);

  await clickButton(page, 'Prompt library');
  await sleep(800);
  if (!page.url().includes('/settings/prompts')) {
    log(`Prompt library click stayed at ${page.url()}, navigating directly`);
    await page.goto(`${APP_URL}/settings/prompts`, { waitUntil: 'networkidle2' });
  }
  await page.waitForFunction(() => location.pathname.includes('/settings/prompts'), { timeout: 15000 });
  await waitForText(page, 'Loading prompts', 8000).catch(() => null);
  await waitForText(page, 'Duplicate to edit', 20000);
  await sleep(800);
  await page.reload({ waitUntil: 'networkidle2' });
  await waitForText(page, 'Duplicate to edit', 20000);
  await page.waitForFunction(
    () => {
      const item = Array.from(document.querySelectorAll('ion-item')).find((el) =>
        (el.textContent || '').includes('development.reviewer'),
      );
      return !!(item && /open editor/i.test(item.textContent || ''));
    },
    { timeout: 20000 },
  );
  await sleep(400);
  let info = await pageInfo(page);
  note(
    `S1 library url=${info.url} title=${info.title} rows~=${info.rows} keys=${info.keys.join(',')} duplicate=${info.duplicate} open=${info.openEd} hint=${info.hint} inert=${info.inert} used=${info.used.join(',')}`,
  );
  await shot(page, 'prompt-library-populated-mobile', 390, 844);
  // Tall viewport from the top: hint + one-row filters + all shipped keys in one PNG.
  // (ion-content internal scroll makes page.screenshot({fullPage:true}) clip to 800.)
  await shot(page, 'prompt-library-populated-desktop', 1280, 1600);
  await page.goto(`${APP_URL}/settings/prompts`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Duplicate to edit', 20000);
  await page.setViewport({ width: 1280, height: 800 });
  await sleep(400);

  await selectFilter(page, 'Filter by role', 'lens');
  await sleep(600);
  info = await pageInfo(page);
  note(`S2 role=lens rows~=${info.rows} keys=${info.keys.join(',')} textHasPlanner=${info.snippet.includes('Planning planner')}`);
  await shot(page, 'prompt-library-filter-lens', 1280, 800);

  await selectFilter(page, 'Filter by role', 'All roles');
  await selectFilter(page, 'Filter by scope', 'project');
  await sleep(600);
  info = await pageInfo(page);
  note(`S3 scope=project empty=${info.empty} url=${info.url}`);
  await shot(page, 'prompt-library-scope-project-empty', 1280, 800);

  await selectFilter(page, 'Filter by scope', 'All scopes');
  await typeSearch(page, 'zzzz-no-match');
  await sleep(800);
  info = await pageInfo(page);
  note(`S4 search zzzz-no-match empty=${info.empty}`);
  await shot(page, 'prompt-library-search-miss', 1280, 800);

  await typeSearch(page, '');
  await page.evaluate(() => {
    const input = document.querySelector('ion-searchbar input');
    if (input) {
      input.value = '';
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await sleep(500);

  const dupHref = await page.evaluate(() => {
    const btn = Array.from(document.querySelectorAll('ion-button')).find((b) =>
      (b.textContent || '').includes('Duplicate to edit'),
    );
    if (!btn) return null;
    btn.click();
    return true;
  });
  await sleep(1200);
  note(`S7 Duplicate clicked=${dupHref} url=${page.url()}`);
  const hasTextarea = await page.evaluate(() => !!document.querySelector('ion-textarea, textarea'));
  note(`S7 editor textarea=${hasTextarea}`);

  await page.goto(`${APP_URL}/settings/prompts`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Duplicate to edit', 20000);
  const opened = await page.evaluate(() => {
    const items = Array.from(document.querySelectorAll('ion-item'));
    const item = items.find((el) => (el.textContent || '').includes('development.reviewer'));
    if (!item) return false;
    const btn = Array.from(item.querySelectorAll('ion-button')).find((b) =>
      (b.textContent || '').includes('Open editor'),
    );
    if (!btn) return false;
    (btn.shadowRoot?.querySelector('a, button') || btn).click();
    return true;
  });
  await sleep(1200);
  note(`S8 Open editor on development.reviewer clicked=${opened} url=${page.url()}`);

  await page.goto(`${APP_URL}/settings/prompts`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Customise a built-in via Duplicate', 20000);
  await clickButton(page, '+ New');
  await sleep(1200);
  note(`S11 + New url=${page.url()}`);

  log('restore development.reviewer');
  await page.goto(`${APP_URL}/settings?prompt=development.reviewer`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Planner prompts');
  await waitForText(page, 'Development reviewer', 25000);
  await page.waitForSelector('ion-accordion', { timeout: 15000 });
  await sleep(600);
  await resetReviewer(page);
  await page.goto(`${APP_URL}/settings?prompt=development.reviewer`, { waitUntil: 'networkidle2' });
  await waitForText(page, 'Planner prompts');
  await waitForText(page, 'Development reviewer', 25000);
  await sleep(600);
  badge = await badgeFor(page, 'development.reviewer');
  log(`badge after restore+reload: ${badge}`);
  note(`restore development.reviewer, reload Settings badge="${badge}"`);

  fs.writeFileSync(path.join(OUT_DIR, 'capture-log.txt'), logLines.join('\n') + '\n');
  fs.writeFileSync(
    path.join(OUT_DIR, 'live-path.md'),
    `# Live path — Prompt Library\n\n${live.join('\n')}\n`,
  );
  await browser.close();
  log('done');
})().catch((err) => {
  console.error(err);
  fs.writeFileSync(
    path.join(OUT_DIR, 'capture-log.txt'),
    logLines.join('\n') + `\nFAILED ${err.stack || err}\n`,
  );
  process.exit(1);
});
