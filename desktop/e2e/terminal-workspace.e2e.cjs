const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.JOHNNYONE_E2E_URL || 'https://johnnyone-dev-web.pages.dev';
const WORKER_URL = process.env.JOHNNYONE_E2E_WORKER_URL || '';
const ARTIFACT_DIR = process.env.JOHNNYONE_E2E_ARTIFACT_DIR
  || path.resolve(__dirname, 'artifacts/terminal-workspace');

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function screenshot(page, name) {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  const filePath = path.join(ARTIFACT_DIR, `${name}.png`);
  await page.screenshot({ path: filePath, fullPage: true });
  console.log(`terminal-workspace.e2e: screenshot ${filePath}`);
}

async function count(page, selector) {
  return page.$$eval(selector, (elements) => elements.length);
}

async function waitForPane(page) {
  await page.waitForSelector('.terminal-pane', { timeout: 60000 });
  await page.waitForSelector('.terminal-pane.active johnny-terminal-screen .xterm-rows', { timeout: 60000 });
  await page.waitForFunction(
    () => (document.querySelector('.terminal-pane.active .pane-status')?.textContent || '')
      .toLowerCase()
      .includes('attached'),
    { timeout: 60000 },
  );
}

async function openNewTerminal(page) {
  const before = await count(page, '.terminal-pane');
  await page.click('.workspace-actions .primary-action');
  await page.waitForFunction(
    (previousCount) => document.querySelectorAll('.terminal-pane').length > previousCount,
    { timeout: 60000 },
    before,
  );
  await waitForPane(page);
}

async function focusActiveTerminal(page) {
  await page.waitForSelector('.terminal-pane.active johnny-terminal-screen .xterm-helper-textarea', { timeout: 15000 });
  await page.click('.terminal-pane.active johnny-terminal-screen');
  await page.$eval('.terminal-pane.active johnny-terminal-screen .xterm-helper-textarea', (element) => element.focus());
}

async function assertTerminalFits(page) {
  const geometry = await page.evaluate(() => {
    const pane = document.querySelector('.terminal-pane.active');
    const host = document.querySelector('.terminal-pane.active johnny-terminal-screen .terminal-host');
    const xterm = document.querySelector('.terminal-pane.active johnny-terminal-screen .xterm');
    const screen = document.querySelector('.terminal-pane.active johnny-terminal-screen .xterm-screen');
    const rows = document.querySelector('.terminal-pane.active johnny-terminal-screen .xterm-rows');
    if (!pane || !host || !xterm || !screen || !rows) return null;

    const rect = (element) => {
      const r = element.getBoundingClientRect();
      return {
        left: r.left,
        right: r.right,
        top: r.top,
        bottom: r.bottom,
        width: r.width,
        height: r.height,
      };
    };

    const visibleRows = Array.from(rows.children)
      .map((row) => ({ text: row.textContent || '', rect: rect(row) }))
      .filter((row) => row.text.trim().length > 0);

    return {
      pane: rect(pane),
      host: rect(host),
      xterm: rect(xterm),
      screen: rect(screen),
      lastVisibleRow: visibleRows.at(-1) || null,
    };
  });

  assert(geometry, 'Terminal geometry was not available');
  assert(geometry.xterm.bottom <= geometry.host.bottom + 2, `xterm overflows host bottom: ${JSON.stringify(geometry)}`);
  assert(geometry.xterm.right <= geometry.host.right + 2, `xterm overflows host right: ${JSON.stringify(geometry)}`);
  assert(
    !geometry.lastVisibleRow || geometry.lastVisibleRow.rect.bottom <= geometry.pane.bottom + 2,
    `last visible row is clipped by pane bottom: ${JSON.stringify(geometry)}`,
  );
}

async function assertWheelScroll(page) {
  const hostSelector = '.terminal-pane.active johnny-terminal-screen .terminal-host';
  const rowsSelector = '.terminal-pane.active johnny-terminal-screen .xterm-rows';
  await page.waitForSelector(hostSelector, { timeout: 15000 });
  await page.waitForSelector(rowsSelector, { timeout: 15000 });

  const before = await page.$eval(rowsSelector, (element) => element.textContent || '');

  await page.hover(hostSelector);
  await page.mouse.wheel({ deltaY: -900 });
  await delay(350);

  const after = await page.$eval(rowsSelector, (element) => element.textContent || '');

  assert(before !== after, 'mouse wheel did not change visible terminal rows');
}

async function terminalText(page) {
  return page.$eval(
    '.terminal-pane.active johnny-terminal-screen .xterm-rows',
    (element) => element.textContent || '',
  );
}

async function cursorLeft(page) {
  return page.$eval(
    '.terminal-pane.active johnny-terminal-screen .xterm-cursor',
    (element) => element.getBoundingClientRect().left,
  );
}

function countOccurrences(text, needle) {
  return text.split(needle).length - 1;
}

async function run() {
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });

  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });

  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1440, height: 900 });

    if (WORKER_URL) {
      await page.evaluateOnNewDocument((workerUrl) => {
        window.localStorage.setItem('johnnyone_worker_url', workerUrl);
      }, WORKER_URL);
    }

    page.on('console', (message) => {
      if (message.type() === 'error') {
        console.error(`[browser:${message.type()}] ${message.text()}`);
      }
    });
    page.on('pageerror', (error) => {
      console.error(`[browser:pageerror] ${error.message}`);
    });

    console.log(`terminal-workspace.e2e: open ${APP_URL}/chat`);
    await page.goto(`${APP_URL.replace(/\/$/, '')}/chat`, { waitUntil: 'networkidle2', timeout: 60000 });
    await page.waitForSelector('.workspace-shell', { timeout: 30000 });

    if ((await count(page, '.terminal-pane')) === 0) {
      await openNewTerminal(page);
    } else {
      await waitForPane(page);
    }

    await screenshot(page, '01-initial-terminal');
    await assertTerminalFits(page);

    const tabsBeforeClose = await count(page, '.workspace-tab');
    const panesBeforeClose = await count(page, '.terminal-pane');
    assert(tabsBeforeClose > 0, 'Expected at least one terminal tab before close');
    assert(panesBeforeClose > 0, 'Expected at least one terminal pane before close');

    console.log('terminal-workspace.e2e: close active pane');
    await page.click('.terminal-pane.active .pane-icon');
    await page.waitForFunction(
      ({ tabsBefore, panesBefore }) =>
        document.querySelectorAll('.workspace-tab').length === tabsBefore - 1
        && document.querySelectorAll('.terminal-pane').length === panesBefore - 1,
      { timeout: 15000 },
      { tabsBefore: tabsBeforeClose, panesBefore: panesBeforeClose },
    );
    await screenshot(page, '02-after-close');

    console.log('terminal-workspace.e2e: open replacement terminal');
    await openNewTerminal(page);
    await screenshot(page, '03-after-new-terminal');
    await assertTerminalFits(page);

    console.log('terminal-workspace.e2e: verify wheel scroll');
    await focusActiveTerminal(page);
    await page.keyboard.type('space-reflect');
    await delay(1200);
    const cursorBeforeSpace = await cursorLeft(page);
    await page.keyboard.press('Space');
    await delay(1200);
    const cursorAfterSpace = await cursorLeft(page);
    const spaceText = await terminalText(page);
    assert(spaceText.includes('space-reflect'), 'typed text did not render before Enter');
    assert(cursorAfterSpace > cursorBeforeSpace, 'typed trailing space did not move the visible cursor before Enter');
    await page.keyboard.down('Control');
    await page.keyboard.press('KeyU');
    await page.keyboard.up('Control');
    await delay(800);

    console.log('terminal-workspace.e2e: verify typing after final response stays visible');
    const promptCountBefore = countOccurrences(await terminalText(page), '>>>');
    await page.keyboard.type('hi');
    await page.keyboard.press('Enter');
    await page.waitForFunction(
      (previousCount) => {
        const text = document.querySelector('.terminal-pane.active johnny-terminal-screen .xterm-rows')?.textContent || '';
        return text.split('>>>').length - 1 > previousCount;
      },
      { timeout: 45000 },
      promptCountBefore,
    );
    await page.keyboard.type('post-final-visible');
    await page.keyboard.press('Space');
    await delay(1200);
    const postFinalText = await terminalText(page);
    assert(postFinalText.includes('post-final-visible '), 'typing after final response was not visible before Enter');

    await page.keyboard.type('hello terminal scroll test');
    await page.keyboard.press('Enter');
    await delay(3000);
    await assertWheelScroll(page);
    await screenshot(page, '04-after-wheel-scroll');

    const finalTabs = await count(page, '.workspace-tab');
    if (finalTabs > 0) {
      await page.click('.terminal-pane.active .pane-icon');
      await page.waitForFunction(
        (previousCount) => document.querySelectorAll('.workspace-tab').length === previousCount - 1,
        { timeout: 15000 },
        finalTabs,
      );
    }

    console.log('terminal-workspace.e2e: passed');
  } finally {
    await browser.close();
  }
}

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
