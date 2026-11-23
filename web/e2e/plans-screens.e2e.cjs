#!/usr/bin/env node
/**
 * Plans tab — screen capture with FIXTURE data (initiative 8ad3ae71, phase 03; Product read-back).
 *
 * Drives the REAL built web app in a real Chrome. The login goes to the real worker; ONLY the
 * plans-tab GraphQL operations (listDraftPlans, getAgentPlan, getBriefVersion, listBriefVersions,
 * getBriefCheck, getBriefDiff, getAiSession, listAiMessages, sendRelayChatMessage) are answered
 * from the fixture below via Puppeteer request interception. Every pixel is the shipped Angular/
 * Ionic UI; only the data is a fixture, modelled on the 08 mock's initiative (v1 you … v5 agent).
 * Use it to read back screens S1-S11 before the plans API is deployed; `plans-smoke.e2e.cjs` is the
 * live end-to-end run against a deployed worker + engine (D10: human-run).
 *
 * Usage (serve the bundle first, e.g. `npx --yes serve@14 -s dist/web/browser -l 4280`):
 *   APP_URL=http://127.0.0.1:4280 OUT_DIR=/tmp/plans-shots node web/e2e/plans-screens.e2e.cjs
 */
const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const APP_URL = process.env.APP_URL || 'http://127.0.0.1:4280';
const EMAIL = process.env.JOHNNYONE_EMAIL || 'admin@johnnyone.local';
const PASSWORD = process.env.JOHNNYONE_PASSWORD || 'johnnyone-dev';
const TENANT_ID = process.env.JOHNNYONE_TENANT || '00000000-0000-0000-0000-000000000001';
const OUT_DIR = process.env.OUT_DIR || path.join(__dirname, '__screenshots__', 'plans');
fs.mkdirSync(OUT_DIR, { recursive: true });

const log = (...a) => console.log('[plans-screens]', ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── fixture ─────────────────────────────────────────────────────────────────
const NOW = Date.now();
const ago = (min) => new Date(NOW - min * 60_000).toISOString().replace('T', ' ').slice(0, 19);
const PLAN_ID = '8f1c2d3e-0000-4000-8000-000000000005';
const SESSION_ID = 'sess-plan-agent-0005';
const WS = '/home/creepy/Documents/Workspace/personal/apps/johnnyone';

const versions = [
  { version: 1, author: 'you', changeNote: 'seeded from the ask', changed: ['outcome', 'why'], createdAt: ago(9) },
  { version: 2, author: 'agent', changeNote: '', changed: ['outcome'], createdAt: ago(8) },
  { version: 3, author: 'agent', changeNote: '', changed: ['why', 'where', 'out_of_scope'], createdAt: ago(8) },
  { version: 4, author: 'agent', changeNote: '', changed: ['done_when', 'decided', 'outline:00/04'], createdAt: ago(1) },
  { version: 5, author: 'agent', changeNote: '', changed: ['out_of_scope'], createdAt: ago(0) },
];
const fields = {
  outcome: 'Every agent tmux session is killed and archived when its job ends; orphans are swept at engine start and every 10 min.',
  why: 'Planner johnnyone_1d5fdf55 and dev worker johnnyone_67686e3b were still running claude a day after their initiative finished.\nLens reviewers are fine (dispose_ephemeral_agent).',
  where: ['desktop/src-tauri/src/services/agent_plans.rs', 'desktop/src-tauri/src/terminal.rs'],
  doneWhen: 'cargo test -j 8 --lib → 0 failed, 3 runs\ndecision fn never selects j1 / attached / active worker\nretired session is archived and not resurrected',
  outOfScope: 'Lens disposal (already correct)\nUI changes\nanything not named johnnyone_<uuid> (never guard / kord / j1)',
  decided: 'Retire = kill + archive (keep the transcript). Archived sessions are never resurrected.',
};
const outline = {
  phases: [
    {
      id: '00', title: 'Retire on job end', author: 'agent', addedIn: 2, nextTask: 5,
      tasks: [
        { id: '00/01', title: 'retire helper (kill + archive + event)', author: 'agent', addedIn: 2 },
        { id: '00/02', title: 'retire planner on approval', author: 'agent', addedIn: 2 },
        { id: '00/03', title: 'retire worker on completion', author: 'you', addedIn: 3 },
        { id: '00/04', title: 'swap & revive: keep Amend / Run-from-phase working', author: 'agent', addedIn: 4 },
      ],
    },
    {
      id: '01', title: 'Orphan sweep (start + every 10 min)', author: 'agent', addedIn: 3, nextTask: 3,
      tasks: [
        { id: '01/01', title: 'decision function: never j1 / kord / guard / attached', author: 'agent', addedIn: 3 },
        { id: '01/02', title: 'sweep executor (archives, never just kills)', author: 'agent', addedIn: 3 },
      ],
    },
  ],
  nextPhase: 2,
};
const latest = (status = 'briefing', accepted = null) => ({
  initiativeId: PLAN_ID, version: 5, author: 'agent', changeNote: '', changed: ['out_of_scope'], createdAt: ago(0),
  fields, outline, initiativeStatus: status, acceptedBriefVersion: accepted,
});
const checks = [
  { rule: 'outcome_one_sentence', status: 'ok', detail: 'Outcome is one sentence', paths: [], initiativeIds: [] },
  { rule: 'done_when_runnable', status: 'ok', detail: '1 runnable check(s) in Done when', paths: [], initiativeIds: [] },
  { rule: 'out_of_scope_present', status: 'ok', detail: 'Out of scope is stated', paths: [], initiativeIds: [] },
  { rule: 'where_exists', status: 'ok', detail: 'All 2 Where path(s) exist', paths: [], initiativeIds: [] },
  { rule: 'file_conflict', status: 'warn', detail: 'Findings ledger touches desktop/src-tauri/src/services/agent_plans.rs', paths: ['desktop/src-tauri/src/services/agent_plans.rs'], initiativeIds: ['ledger-1'] },
];
const plan = (status = 'briefing', accepted = null) => ({
  id: PLAN_ID, runType: 'planning', title: 'Retire agent tmux sessions + orphan sweep', workspacePath: WS,
  planPath: '/x/plan', status: status === 'briefing' ? 'draft' : 'planning_planner_running', initiativeId: PLAN_ID,
  initiativeStatus: status, health: 'in-progress', briefingSessionId: SESSION_ID, acceptedBriefVersion: accepted,
  brief: '# Outcome', planningQueuedAfter: null, workerProvider: 'claude_code', reviewerProvider: 'claude_code',
  currentPhaseIndex: 0, createdAt: ago(9), updatedAt: ago(0),
});
const tc = (id, name, input, result) => ({ id, name: `mcp__j1__${name}`, input, result });
const messages = [
  { id: 'm1', sessionId: SESSION_ID, role: 'user', content: "The planner and dev worker tmux sessions stay alive after the initiative finishes. Let's fix that.", createdAt: new Date(NOW - 9 * 60_000).toISOString(), inputTokens: 0, outputTokens: 0, costCents: 0 },
  {
    id: 'm2', sessionId: SESSION_ID, role: 'assistant', createdAt: new Date(NOW - 8 * 60_000).toISOString(), inputTokens: 0, outputTokens: 0, costCents: 0,
    content: "Confirmed in the code: lens reviewers are disposed after every round, but the planner and the dev worker are only killed on stop/delete. I've drafted the brief.",
    toolCalls: JSON.stringify([
      tc('t1', 'repo_search', { query: 'kill_terminal_session' }, JSON.stringify({ matches: [1, 2, 3, 4, 5, 6, 7].map((n) => ({ path: 'a.rs', line: n, text: 'x' })), truncated: false, timedOut: false, excluded: false })),
      tc('t2', 'brief_update', { field: 'outcome', value: fields.outcome }, '{"version":2,"changed_fields":["outcome"]}'),
      tc('t3', 'brief_update', { field: 'why', value: fields.why }, '{"version":3,"changed_fields":["why","where"]}'),
      tc('t4', 'brief_update', { field: 'out_of_scope', add: 'lens disposal (already correct)' }, '{"version":3,"changed_fields":["out_of_scope"]}'),
    ]),
  },
  { id: 'm3', sessionId: SESSION_ID, role: 'user', content: "After a reboot or an engine restart they come back. Killing isn't enough. And never touch guard, kord or j1.", createdAt: new Date(NOW - 2 * 60_000).toISOString(), inputTokens: 0, outputTokens: 0, costCents: 0 },
  {
    id: 'm4', sessionId: SESSION_ID, role: 'assistant', createdAt: new Date(NOW - 60_000).toISOString(), inputTokens: 0, outputTokens: 0, costCents: 0,
    content: 'Right: `ensure_terminal_session` resurrects any session that is not archived. So retire = kill **+ archive**, and I added a check for it.\n\nOne warning: `agent_plans.rs` is also being edited by the running **Findings ledger** initiative. Should I queue this one until it ships?\nCHOICES: Queue it after the ledger | Start now | Show me the conflict',
    toolCalls: JSON.stringify([
      tc('t1', 'brief_update', { field: 'decided', add: 'retire = kill + archive' }, '{"version":4,"changed_fields":["decided"]}'),
      tc('t2', 'brief_update', { field: 'done_when', add: 'retired session is not resurrected' }, '{"version":4,"changed_fields":["done_when"]}'),
      tc('t3', 'brief_update', { field: 'out_of_scope', add: 'anything not johnnyone_<uuid>' }, '{"version":5,"changed_fields":["out_of_scope"]}'),
      tc('t4', 'plan_check', {}, JSON.stringify({ items: checks })),
    ]),
  },
];
const draftPlans = [
  { id: PLAN_ID, title: 'Retire agent tmux sessions + orphan sweep', workspacePath: WS, updatedAt: ago(3), latest: { version: 5, author: 'agent', createdAt: ago(3) }, checkSummary: { ok: 4, warn: 1, fail: 0 }, queuedAfter: [] },
  { id: 'p2', title: 'Findings panel in the console', workspacePath: WS, updatedAt: ago(60), latest: { version: 2, author: 'you', createdAt: ago(60) }, checkSummary: { ok: 2, warn: 0, fail: 2 }, queuedAfter: [] },
  { id: 'p3', title: 'Kord export CSV', workspacePath: '/home/creepy/Documents/Workspace/personal/apps/kord', updatedAt: ago(2 * 24 * 60), latest: { version: 1, author: 'you', createdAt: ago(2 * 24 * 60) }, checkSummary: { ok: 0, warn: 0, fail: 3 }, queuedAfter: ['x'] },
];
const diff = {
  fields: [
    { field: 'outcome', changed: false, removed: [], added: [] },
    { field: 'why', changed: false, removed: [], added: [] },
    { field: 'where', changed: false, removed: [], added: [] },
    { field: 'done_when', changed: true, removed: [], added: ['retired session is archived and not resurrected'] },
    { field: 'out_of_scope', changed: false, removed: [], added: [] },
    { field: 'decided', changed: true, removed: [], added: ['Retire = kill + archive (keep the transcript).'] },
  ],
  outline: { added: [{ id: '00/04', title: 'swap & revive: keep Amend / Run-from-phase working' }], removed: [], renamed: [], moved: [] },
};
const dirListing = {
  root: '/home/creepy/Documents/Workspace', path: 'personal/apps',
  entries: ['johnnyone', 'kord', 'guard'].map((n) => ({ name: n, path: `personal/apps/${n}`, kind: 'dir' })),
};

// Mutable per-scenario behaviour.
const mode = { list: 'populated', detail: 'briefing', delayMs: 0 };

function answer(query, variables = {}) {
  const has = (s) => query.includes(s);
  if (has('listDraftPlans')) {
    if (mode.list === 'error') return { errors: [{ message: 'No online backend app found' }] };
    if (mode.list === 'loading') return { delay: 60_000, data: { listDraftPlans: [] } };
    return { data: { listDraftPlans: mode.list === 'empty' ? [] : draftPlans } };
  }
  if (has('getAgentPlan(')) {
    if (mode.detail === 'loading') return { delay: 60_000, data: null };
    if (mode.detail === 'error') return { errors: [{ message: 'No online backend app found' }] };
    if (mode.detail === 'not-found') return { errors: [{ message: `plan ${PLAN_ID} not found` }] };
    const frozen = mode.detail === 'frozen';
    return { data: { getAgentPlan: { plan: plan(frozen ? 'planning' : 'briefing', frozen ? 5 : null), phases: [], tasks: [], events: [] } } };
  }
  const frozen = mode.detail === 'frozen';
  if (has('getBriefVersion')) return { data: { getBriefVersion: latest(frozen ? 'planning' : 'briefing', frozen ? 5 : null) } };
  if (has('listBriefVersions')) return { data: { listBriefVersions: versions } };
  if (has('getBriefCheck')) return { data: { getBriefCheck: checks } };
  if (has('getBriefDiff')) return { data: { getBriefDiff: diff } };
  if (has('getAiSession')) return { data: { getAiSession: { id: SESSION_ID, title: 'Plan agent', provider: 'claude_code', model: '', workingDirectory: WS, status: 'active', totalInputTokens: 0, totalOutputTokens: 0, totalCostCents: 0, createdAt: ago(9), updatedAt: ago(0) } } };
  if (has('listAiMessages')) return { data: { listAiMessages: messages } };
  if (has('sendRelayChatMessage')) return { data: { sendRelayChatMessage: { success: true, relayId: 'relay-fixture-1', desktopNodeId: 'n1' } } };
  if (has('filesListDir')) {
    // Path-aware: answer the folder actually requested, so navigating changes what the picker shows.
    const rel = String(variables.path ?? dirListing.path).replace(/^\/+|\/+$/g, '');
    if (rel === '' || rel === dirListing.path) return { data: { filesListDir: dirListing } }; // the initial browse lands in apps/
    return { data: { filesListDir: { root: dirListing.root, path: rel, entries: ['desktop', 'web', 'worker', 'ui'].map((n) => ({ name: n, path: `${rel}/${n}`, kind: 'dir' })) } } };
  }
  return null;
}

async function shot(page, name, opts = {}) {
  await sleep(opts.wait ?? 900);
  const file = path.join(OUT_DIR, `${name}.png`);
  await page.screenshot({ path: file, fullPage: false });
  log(`screenshot: ${file}`);
}

async function clickText(page, selector, text) {
  const ok = await page.evaluate(
    (sel, t) => {
      const visible = (el) => !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length) && !el.closest('.ion-page-hidden');
      const el = Array.from(document.querySelectorAll(sel)).find((e) => visible(e) && (e.textContent || '').includes(t));
      if (!el) return false;
      el.click();
      return true;
    },
    selector,
    text,
  );
  if (!ok) throw new Error(`no visible ${selector} containing "${text}"`);
}

async function go(page, url) {
  await page.goto(`${APP_URL}${url}`, { waitUntil: 'networkidle2', timeout: 30000 }).catch(() => undefined);
  await sleep(1200);
}

(async () => {
  const browser = await puppeteer.launch({ headless: 'new', args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 860 });
  // Cloudflare refuses a HeadlessChrome UA on the worker (error 1010), so look like a normal browser.
  await page.setUserAgent((await browser.userAgent()).replace('HeadlessChrome', 'Chrome'));
  page.on('pageerror', (e) => log('pageerror:', e.message));
  // A locally served bundle defaults to a local worker (127.0.0.1:7714); log in against the real one.
  const WORKER_URL = process.env.WORKER_URL || 'https://johnnyone.ethan-353.workers.dev';
  await page.evaluateOnNewDocument((u) => {
    try {
      localStorage.setItem('johnnyone_worker_url', u);
    } catch {
      /* about:blank has no storage */
    }
  }, WORKER_URL);
  await page.setRequestInterception(true);
  page.on('request', async (req) => {
    try {
      if (req.url().endsWith('/graphql') && req.method() === 'POST') {
        const body = JSON.parse(req.postData() || '{}');
        const a = answer(String(body.query || ''), body.variables || {});
        if (a) {
          if (a.delay) await sleep(a.delay);
          return req.respond({
            status: 200,
            contentType: 'application/json',
            headers: { 'access-control-allow-origin': '*' },
            body: JSON.stringify(a.errors ? { errors: a.errors } : { data: a.data }),
          });
        }
      }
      return req.continue();
    } catch {
      try { req.continue(); } catch { /* already handled */ }
    }
  });

  // ── real login ──
  await go(page, '/login');
  await page.waitForSelector('ion-button', { timeout: 20000 });
  for (const [label, value] of [['Email', EMAIL], ['Password', PASSWORD], ['Tenant', TENANT_ID]]) {
    const h = await page.evaluateHandle((l) => {
      const item = Array.from(document.querySelectorAll('ion-item')).find((el) => (el.textContent || '').includes(l));
      return item ? item.querySelector('input') : null;
    }, label);
    if (await h.evaluate((el) => !!el)) {
      await h.click({ clickCount: 3 });
      await h.type(value);
    }
  }
  await clickText(page, 'ion-button', 'Sign in');
  await page.waitForFunction(() => !location.pathname.startsWith('/login'), { timeout: 30000 });
  log('logged in:', page.url());

  // S1 + S2: nav + populated list (desktop rail).
  await go(page, '/plans');
  await shot(page, 's1-s2-plans-list-desktop');
  // S3: empty / error / loading.
  mode.list = 'empty';
  await go(page, '/plans');
  await shot(page, 's3-plans-empty');
  mode.list = 'error';
  await go(page, '/plans');
  await shot(page, 's3-plans-error');
  mode.list = 'loading';
  await go(page, '/plans');
  await shot(page, 's3-plans-loading', { wait: 300 });
  mode.list = 'populated';
  await go(page, '/plans');
  // S4: New plan modal + the shared picker.
  await clickText(page, 'ion-button', 'New plan');
  await sleep(900);
  const title = await page.$('ion-modal ion-input input');
  if (title) await title.type('Plans tab');
  await shot(page, 's4-new-plan-modal');
  await clickText(page, 'ion-item', 'Workspace');
  await shot(page, 's4-new-plan-picker');
  // No silent catches: if either step fails the capture fails, rather than re-shooting the picker.
  await clickText(page, 'ion-item', 'johnnyone');
  await page.waitForFunction(() => (document.querySelector('app-host-dir-picker .dir-path')?.textContent || '').includes('johnnyone'), { timeout: 10000 });
  await sleep(400);
  await clickText(page, 'ion-button', 'Use this folder');
  await page.waitForFunction(
    () => Array.from(document.querySelectorAll('.ws-value')).some((e) => (e.textContent || '').includes('personal/apps/johnnyone')),
    { timeout: 10000 },
  );
  await shot(page, 's4-new-plan-filled');

  // S5: detail, desktop.
  await go(page, `/plans/${PLAN_ID}`);
  await shot(page, 's5-plan-detail-desktop', { wait: 1500 });
  // The brief panel scrolls inside its own container (a fullPage shot repeats the top), so scroll the
  // panel itself down to the brief check + draft outline for a distinct second desktop claim.
  const scrolled = await page.evaluate(async () => {
    const target = document.querySelector('.checks');
    if (!target) return false;
    // Nearest scrollable ancestor of the outline (the brief column scrolls on its own).
    let el = target.parentElement;
    while (el && !(el.scrollHeight > el.clientHeight + 4 && /auto|scroll/.test(getComputedStyle(el).overflowY))) el = el.parentElement;
    if (!el) {
      const content = Array.from(document.querySelectorAll('ion-content')).find((c) => c.offsetWidth);
      el = content ? await content.getScrollElement() : null;
    }
    if (!el) return false;
    el.scrollTop += target.getBoundingClientRect().top - el.getBoundingClientRect().top - 260;
    window.__shotScroller = el;
    return el.scrollTop > 0;
  });
  if (!scrolled) throw new Error('could not scroll the detail page to the brief check + outline');
  await shot(page, 's5-plan-detail-desktop-check-outline');
  await page.evaluate(() => { if (window.__shotScroller) window.__shotScroller.scrollTop = 0; });
  // S7: diff of v4.
  await clickText(page, 'app-version-chip', 'v4');
  await shot(page, 's7-version-diff');
  await clickText(page, 'ion-button', 'Back to latest');
  // S8: direct edit of Done when.
  await page.evaluate(() => {
    const f = Array.from(document.querySelectorAll('app-brief-field')).find((e) => (e.textContent || '').includes('Done when'));
    // Ionic moves aria-label from the ion-button host onto its inner button, so find it by icon.
    f?.querySelector('ion-icon[name="create-outline"]')?.closest('ion-button')?.click();
  });
  await shot(page, 's8-direct-edit');
  // S10: busy — tap a suggestion chip; the send is accepted but no reply streams back, so the
  // composer stays disabled with the streaming indicator.
  await clickText(page, 'ion-chip', 'Start now');
  await shot(page, 's10-agent-busy');

  // S6: phone, the three panes.
  await page.setViewport({ width: 400, height: 860, isMobile: true, hasTouch: true });
  await go(page, `/plans/${PLAN_ID}`);
  await shot(page, 's6-phone-chat', { wait: 1500 });
  await clickText(page, 'ion-segment-button', 'Brief');
  await shot(page, 's6-phone-brief');
  await clickText(page, 'ion-segment-button', 'Outline');
  await shot(page, 's6-phone-outline');
  await go(page, '/plans');
  await shot(page, 's1-s2-plans-list-phone');

  // S9: frozen; S11: load states.
  await page.setViewport({ width: 1280, height: 860 });
  mode.detail = 'frozen';
  await go(page, `/plans/${PLAN_ID}`);
  await shot(page, 's9-frozen', { wait: 1500 });
  mode.detail = 'error';
  await go(page, `/plans/${PLAN_ID}`);
  await shot(page, 's11-detail-error');
  mode.detail = 'not-found';
  await go(page, `/plans/${PLAN_ID}`);
  await shot(page, 's11-detail-not-found');
  mode.detail = 'loading';
  await page.goto(`${APP_URL}/plans/${PLAN_ID}`, { waitUntil: 'domcontentloaded' }).catch(() => undefined);
  await shot(page, 's11-detail-loading', { wait: 2500 });

  await browser.close();
  log('✓ done:', OUT_DIR);
  process.exit(0);
})().catch((err) => {
  console.error('✗ capture failed:', err.message);
  process.exit(1);
});
