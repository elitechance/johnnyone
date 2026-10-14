// Proves the arena has a FRONT DOOR, that the nav says where you are, that the URL does not lose
// the initiative on the way back, and that a stale link does not print a stack trace. Real Chrome,
// the BUILT bundle, AT BOTH WIDTHS. Exits non-zero when any of it is untrue.
//
// WHY IT IS A SEPARATE HARNESS from `arena-phone-harness.mjs`:
//   * that one gets to an arena page by TYPING A URL, which is the one thing the front-door check
//     is not allowed to do — the check is "a person starting at `/` arrives having only clicked";
//   * its GraphQL stub answers with the SAME workflow for every id, so it structurally cannot see
//     an id-routing bug or an unknown id, and it has exactly ONE initiative, so it cannot see the
//     initiative-context loss, which is only visible once two initiatives exist;
//   * it runs at 400x860 only, and the biggest seam is on the DESKTOP: measured at 1440x900 the
//     console is rail 64 + list 216 + centre 886 + validation 262, and the arena was a single
//     1100px column — crossing discarded 478px of persistent chrome.
//
// FOUR WAYS THIS HARNESS COULD HAVE LIED, all closed deliberately:
//   1. ASSERTING CLASSES ON A HIDDEN ELEMENT. `.botnav` keeps its `on` class above 900px where it
//      is `display: none`, and `.gnav` keeps its own below it. An earlier measurement of this very
//      feature quoted `"ni on"` as the bottom nav's class list — it was the hidden desktop rail.
//      Every nav check here NAMES the surface for the width and FAILS if that surface is not
//      painted (`checkVisibility`).
//   2. PROVING THE VIEW AND NOT THE URL. The console component stays mounted in-session, so after
//      clicking back the right initiative is still selected on screen even when the address bar has
//      lost it. The return check therefore RELOADS before it believes anything.
//   3. CLICKING SOMETHING ONLY A DEVELOPER KNEW WAS THERE. The walk matches on the element's own
//      visible text, because discoverability is half of "there is no way to navigate to this".
//   4. CALLING A CONTROL REACHABLE BECAUSE ARITHMETIC SAYS SO. The occlusion check hit-tests the
//      control's bottom edge against the FIXED bottom nav; a scrollHeight/clientHeight comparison
//      is blind to an overlay and reported 0 unreachable pixels with 6px of the decisive button
//      under the bar.
//
// CI: blocking for `web/src/app/nav-items.ts`, `app.component.*`, `pages/arena/**` and
// `terminal.page.*`; advisory elsewhere. Not wired — the user's call, not a developer's.
//
// Run it:  npx nx run web:arena-nav
//    or:   node web/tools/arena-nav-harness.mjs [serveRoot] [port]
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import puppeteer from 'puppeteer';

const ROOT = process.argv[2] || 'dist/web/browser';
const PORT = Number(process.argv[3] || 8131);
const TYPES = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.ico': 'image/x-icon', '.svg': 'image/svg+xml', '.json': 'application/json', '.woff2': 'font/woff2', '.png': 'image/png' };

// THREE initiatives, and the one we depart from is NOT the one the console auto-selects. With a
// single-initiative fixture every "did we come back to the right initiative" assertion passes
// trivially, because there is only one to come back to.
const plan = (id, title) => ({ id, runType: 'development', title, workspacePath: '/w/johnnyone', planPath: '/x/plan.md', status: 'running', initiativeId: `init-${id}`, initiativeStatus: 'development', health: 'in-progress', currentPhaseId: '04-leases', currentPhaseIndex: 3, phaseRunMode: 'continue', workerProvider: 'claude_code', reviewerProvider: 'claude_code', testCommands: ['npm run check'], createdAt: '2026-10-14T06:00:00Z', updatedAt: '2026-10-14T07:00:00Z' });
const PLANS = [plan('p1', 'First initiative'), plan('p2', 'Second initiative'), plan('p3', 'Third initiative')];
const DEPART_FROM = PLANS[1]; // p2 — deliberately not plans[0], which the console auto-selects.
const phasesFor = (id) => [{ id: `ph-${id}`, planId: id, phaseId: '04-leases', phaseTitle: 'The lease pool', phaseIndex: 3, status: 'running', gateVerdict: 'PENDING', clarificationAttempts: 0, findingsJson: '[]', testCommands: ['cargo test'], effectiveTestCommands: ['cargo test'], createdAt: 'x', updatedAt: 'x' }];

const WF = { id: 'w2', repo: 'johnnyone', phaseType: 'dev-phase', family: 'glm-5.3-flash', version: 2, parentId: 'w1', role: 'r', instruction: 'i', contract: 'c', hop: '{}', fanOut: 2, isChampion: false, createdAt: '2026-10-14T07:00:00Z' };
const CHAMP = { ...WF, id: 'w1', version: 1, parentId: null, fanOut: 1, isChampion: true };
const RUN_PASS = { id: 'r2', workflowId: 'w2', planId: 'p2', seed: 0, gateExit: 0, gateCollected: 23, seatsHeld: '{"glm-5.3-flash":2}', fleetBusy: '{}', startedAt: '2026-10-14T07:00:00Z', endedAt: '2026-10-14T07:06:40Z' };
const RUN_FAIL = { ...RUN_PASS, id: 'r1', workflowId: 'w1', gateExit: 101, gateCollected: null, endedAt: '2026-10-14T07:08:12Z' };
const STATE = { repo: 'johnnyone', phaseType: 'dev-phase', family: 'glm-5.3-flash', consecutiveRejections: 1, halted: false, strikeLimit: 3, recentRejections: [], championWorkflowId: 'w1' };
/** Ids the host knows. Anything else answers `workflow: null` — a stale link or a removed row. */
const KNOWN = { w1: CHAMP, w2: WF };

function answer(body) {
  const b = body || '';
  const name = /(?:query|mutation)\s+(\w+)/.exec(b)?.[1] ?? '';
  // The id the client ASKED for, so an id-routing bug cannot hide behind one canned answer.
  const askedWorkflow = /"id"\s*:\s*"([^"]+)"/.exec(b)?.[1] ?? '';
  const askedPlan = /"(?:planId|id)"\s*:\s*"(p\d)"/.exec(b)?.[1] ?? 'p1';
  const d = {
    GetAgentPlan: { getAgentPlan: { plan: PLANS.find((p) => p.id === askedPlan) ?? PLANS[0], phases: phasesFor(askedPlan), tasks: [], events: [] } },
    ListAgentPlans: { listAgentPlans: PLANS },
    Workflows: { workflows: [CHAMP, WF] },
    Workflow: { workflow: KNOWN[askedWorkflow] ?? null },
    WorkflowRuns: { workflowRuns: askedWorkflow === 'w1' ? [RUN_FAIL] : [RUN_PASS] },
    WorkflowPrefs: { workflowPrefs: [] },
    RepoIdentity: { repoIdentity: 'github.com/lokalcamp/johnnyone' },
    WorkflowSearchState: { workflowSearchState: STATE },
  }[name];
  return JSON.stringify(d ? { data: d } : { data: {} });
}
const server = http.createServer(async (req, res) => {
  if (req.method === 'POST' || req.url.includes('/graphql')) {
    let b = ''; req.on('data', (c) => (b += c));
    req.on('end', () => { res.writeHead(200, { 'content-type': 'application/json', 'access-control-allow-origin': '*' }); res.end(answer(b)); });
    return;
  }
  const rel = decodeURIComponent(req.url.split('?')[0]);
  let file = path.join(ROOT, rel);
  if (!existsSync(file) || rel === '/' || !path.extname(rel)) file = path.join(ROOT, 'index.html');
  try { const data = await readFile(file); res.writeHead(200, { 'content-type': TYPES[path.extname(file)] ?? 'application/octet-stream' }); res.end(data); }
  catch { res.writeHead(404); res.end('no'); }
});
await new Promise((r) => server.listen(PORT, '127.0.0.1', r));

const browser = await puppeteer.launch({ executablePath: '/usr/bin/google-chrome', headless: 'shell', args: ['--no-sandbox', '--disable-dev-shm-usage', '--hide-scrollbars'] });
const page = await browser.newPage();
await page.evaluateOnNewDocument((port) => {
  localStorage.setItem('johnnyone_worker_url', `http://127.0.0.1:${port}`);
  localStorage.setItem('johnnyone_access_token', 'arena-nav-token');
  localStorage.setItem('johnnyone_refresh_token', 'arena-nav-refresh');
  localStorage.setItem('johnnyone_tenant_id', '00000000-0000-0000-0000-000000000001');
  localStorage.setItem('johnnyone_user_id', '00000000-0000-0000-0000-000000000002');
  localStorage.setItem('johnnyone_auth_user', JSON.stringify({ id: 'u1', email: 'ethan@elitechance.com' }));
  localStorage.setItem('johnnyone_token_expires_at', String(Date.now() + 86400000));
}, PORT);

const settle = (ms = 1300) => new Promise((r) => setTimeout(r, ms));
const at = () => page.url().replace(/^http:\/\/[^/]+/, '');
const go = async (u) => { await page.goto(`http://127.0.0.1:${PORT}${u}`, { waitUntil: 'networkidle2' }); await settle(1600); };

/**
 * The two widths, and the nav surface that is actually PAINTED at each. The handover is at 901px
 * (`$mobile-breakpoint: 900px`), and both surfaces keep their `on` class while hidden — which is
 * exactly how a nav assertion becomes a lie.
 */
const WIDTHS = [
  { w: 400, h: 860, mobile: true, surface: 'nav.botnav a.b', surfaceName: 'the bottom bar' },
  { w: 1440, h: 900, mobile: false, surface: '.gnav a.ni', surfaceName: 'the desktop rail' },
];
const setVw = (v) => page.setViewport({ width: v.w, height: v.h, deviceScaleFactor: v.mobile ? 2 : 1, isMobile: v.mobile, hasTouch: v.mobile });

const failures = [];
const log = [];
const fail = (m) => { failures.push(m); console.error(`  FAIL ${m}`); };
const ok = (m) => { log.push(m); console.log(`  ok   ${m}`); };

/** Click a visible element whose OWN text names the arena, repeatedly, until a /workflows URL. */
async function clickToward(re, maxDepth = 6) {
  const trail = [];
  const tried = new Set();
  for (let depth = 0; depth < maxDepth; depth++) {
    const hit = await page.evaluate(({ src, skip }) => {
      const rx = new RegExp(src, 'i');
      const els = [...document.querySelectorAll('a[href],button,[role="tab"]')].filter(
        (el) => el.getClientRects().length && (el.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true),
      );
      const text = (e) => (e.innerText || e.getAttribute('aria-label') || '').trim();
      const el = els.find((e) => rx.test(text(e)) && !skip.includes(`${location.pathname}${location.search}|${text(e).slice(0, 40)}`));
      if (!el) return { found: false, visible: els.length, texts: els.map((e) => text(e).slice(0, 24)) };
      const label = text(el).slice(0, 40);
      el.click();
      return { found: true, label };
    }, { src: re.source, skip: [...tried] });
    if (!hit.found) return { reached: false, trail, visible: hit.visible, texts: hit.texts };
    // Captured BEFORE the click: this is "where they came from". The console's Arena tab is a
    // `?tab=` change, so the departure URL is the one the tab produced, not the walk's first URL.
    const departedFrom = at();
    tried.add(`${at()}|${hit.label}`);
    trail.push(hit.label);
    await settle(1500);
    if (/^\/workflows/.test(at())) return { reached: true, trail, departedFrom };
  }
  return { reached: false, trail, exhausted: true };
}

/** Put the console on a named initiative BY CLICKING, through the mobile master-detail if needed. */
async function selectInitiativeByClick(title) {
  for (let i = 0; i < 3; i++) {
    const r = await page.evaluate((t) => {
      const row = [...document.querySelectorAll('.init-scroll button.init')].find((b) => (b.innerText || '').includes(t));
      if (row && row.getClientRects().length) { row.click(); return 'clicked'; }
      const back = document.querySelector('button.mobile-back');
      if (back && back.getClientRects().length) { back.click(); return 'back'; }
      return 'nothing';
    }, title);
    await settle(1200);
    if (r === 'clicked') return true;
    if (r === 'nothing') return false;
  }
  return false;
}

/** Click a console tab by its label. */
const clickTab = (label) => page.evaluate((l) => {
  const t = [...document.querySelectorAll('.pane-tab')].find((b) => new RegExp(l, 'i').test(b.innerText || ''));
  if (!t) return false; t.click(); return true;
}, label);

// ============ 1. A GLOBAL way in, at both widths, in ONE click from a cold start at `/`.
// This is the claim that survived re-measurement: the in-console route (an initiative's Arena tab)
// always worked, but neither global nav surface had an arena entry at all.
for (const v of WIDTHS) {
  await setVw(v);
  await go('/');
  const r = await page.evaluate((sel) => {
    const items = [...document.querySelectorAll(sel)].filter((a) => a.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true);
    if (!items.length) return { painted: false };
    const arena = items.find((a) => /arena/i.test(`${a.getAttribute('aria-label') ?? ''} ${a.innerText ?? ''}`));
    if (!arena) return { painted: true, found: false, labels: items.map((a) => a.getAttribute('aria-label')) };
    arena.click();
    return { painted: true, found: true };
  }, v.surface);
  await settle(1600);
  const what = `${v.w}px: ${v.surfaceName} has an arena entry, one click from /`;
  if (!r.painted) fail(`${what} — ${v.surfaceName} (${v.surface}) is NOT PAINTED at ${v.w}px, so nothing asserted about it means anything`);
  else if (!r.found) fail(`${what} — no arena entry on ${v.surfaceName}; its items are ${JSON.stringify(r.labels)}`);
  else if (!/^\/workflows/.test(at())) fail(`${what} — the entry was clicked and landed on ${at()}`);
  else ok(`${what} — ${at()}`);
}

// ============ 2. On every arena route, exactly one item is lit on the PAINTED surface, and it is
// the one that names where they are.
const EXPECT_LIT = [
  { route: '/workflows', lit: 'Arena' },
  { route: '/workflows/w2', lit: 'Arena' },
  { route: '/workflows/w2/arena', lit: 'Arena' },
  // Re-measured: this one was never broken. It is a route inside ONE initiative and lights Work.
  { route: '/initiatives/p2/arena?phaseId=04-leases', lit: 'Work' },
  { route: '/initiatives', lit: 'Work' },
];
for (const v of WIDTHS) {
  await setVw(v);
  for (const c of EXPECT_LIT) {
    await go(c.route);
    const r = await page.evaluate((sel) => {
      const items = [...document.querySelectorAll(sel)];
      const painted = items.filter((a) => a.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true);
      return { total: items.length, paintedCount: painted.length, lit: painted.filter((a) => a.classList.contains('on')).map((a) => a.getAttribute('aria-label')) };
    }, v.surface);
    const what = `${v.w}px ${c.route}: ${v.surfaceName} lights exactly "${c.lit}"`;
    if (r.paintedCount === 0) fail(`${what} — ${v.surfaceName} is not painted at this width (${r.total} in the DOM, 0 visible); a class on a hidden element proves nothing`);
    else if (r.lit.length === 1 && r.lit[0] === c.lit) ok(what);
    else fail(`${what} — it lights ${JSON.stringify(r.lit)}`);
  }
}

// ============ 3. The status bar does not paint on one side of the switch and vanish on the other.
// Asserted as PARITY at both widths, because the seam exists at only one: at 400px the console's
// own strip is `display: none` (terminal.page.scss, max-width 900px), so a phone sees none on
// either side and adding one would be inventing chrome the console does not have.
for (const v of WIDTHS) {
  await setVw(v);
  const painted = {};
  for (const route of ['/initiatives', '/workflows', '/workflows/w2', '/workflows/w2/arena', '/initiatives/p2/arena?phaseId=04-leases']) {
    await go(route);
    painted[route] = await page.evaluate(() => [...document.querySelectorAll('footer.workspace-statusbar')].filter((f) => f.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) ?? true).length);
  }
  const base = painted['/initiatives'];
  const odd = Object.entries(painted).filter(([, n]) => (n > 0) !== (base > 0));
  if (odd.length) fail(`${v.w}px: the status bar is ${base > 0 ? 'painted' : 'absent'} on /initiatives but ${JSON.stringify(Object.fromEntries(odd))} across the switch — the chrome changes under you`);
  else ok(`${v.w}px: the status bar is ${base > 0 ? 'painted' : 'absent'} on both sides of the switch`);
}

// ============ 4. THE PRIMARY DEFECT. The URL must not lose the initiative on the way back, and the
// only honest way to check that is to RELOAD: in-session the console component stays mounted, so
// the right initiative is still on screen while the address bar has forgotten it.
for (const v of WIDTHS) {
  await setVw(v);
  await go('/initiatives');
  if (!(await selectInitiativeByClick(DEPART_FROM.title))) { fail(`${v.w}px: could not click initiative "${DEPART_FROM.title}" in the console list`); continue; }
  if (!(await clickTab('arena'))) { fail(`${v.w}px: no Arena tab on the console after selecting "${DEPART_FROM.title}"`); continue; }
  await settle(1300);
  const departedFrom = at();
  const inward = await clickToward(/workflow|arena/, 4);
  if (!inward.reached) { fail(`${v.w}px: could not click from ${departedFrom} into the arena (trail ${JSON.stringify(inward.trail)})`); continue; }
  // WHICH door the walk used matters, and it differs by width: at 400px the global bar is below the
  // fold of the DOM order so it takes the console's Arena tab, and at 1440px the rail comes first so
  // it takes the GLOBAL entry. Both must preserve the initiative, which is why the trail is named in
  // every message here rather than assumed.
  const landed = at();
  let steps = 0;
  for (let i = 0; i < 4; i++) {
    const clicked = await page.evaluate(() => { const b = document.querySelector('a.a-back'); if (!b) return false; b.click(); return true; });
    if (!clicked) break;
    steps++;
    await settle(1200);
    if (at().startsWith('/initiatives')) break;
  }
  const back = at();
  if (back !== departedFrom) {
    fail(`${v.w}px: departed ${departedFrom} via ${JSON.stringify(inward.trail)}, went to ${landed}, and ${steps} back-click(s) put ${back} in the address bar — not where they came from`);
    continue;
  }
  // AND IT HAS TO SURVIVE A RELOAD. This is the part no DOM check in the same session can see.
  await page.reload({ waitUntil: 'networkidle2' });
  await settle(1800);
  const after = await page.evaluate(() => ({
    selected: (document.querySelector('.init-row.sel .t') ?? document.querySelector('button.init.sel .t'))?.textContent?.trim() ?? null,
    tab: [...document.querySelectorAll('.pane-tab')].find((b) => b.classList.contains('on'))?.innerText?.trim() ?? null,
  }));
  if (after.selected === DEPART_FROM.title && /arena/i.test(after.tab ?? '')) {
    ok(`${v.w}px: departed ${departedFrom} via ${JSON.stringify(inward.trail)} -> ${landed} -> back in ${steps} step(s), and a RELOAD of that URL still shows "${after.selected}" on its "${after.tab}" tab`);
  } else {
    fail(`${v.w}px: back put ${back} in the address bar, but RELOADING it shows initiative ${JSON.stringify(after.selected)} on tab ${JSON.stringify(after.tab)} — expected "${DEPART_FROM.title}" on Arena. The view was right only because the component never unmounted.`);
  }
}

// ============ 5. A stale link is an absence, not a stack trace.
for (const v of WIDTHS) {
  await setVw(v);
  for (const route of ['/workflows/w9', '/workflows/w9/arena']) {
    await go(route);
    const r = await page.evaluate(() => ({
      text: document.body.innerText || '',
      wayOut: !!document.querySelector('[data-test="not-found-out"], a.a-back'),
    }));
    const leak = /TypeError|Cannot read properties|undefined is not|\bat \w+\.\w+ \(/.exec(r.text);
    if (leak) fail(`${v.w}px ${route}: an unknown id prints an internal error as body copy — ${JSON.stringify(r.text.replace(/\s+/g, ' ').slice(0, 160))}`);
    else if (!/not here|not found|removed/i.test(r.text)) fail(`${v.w}px ${route}: an unknown id says nothing about what happened — ${JSON.stringify(r.text.replace(/\s+/g, ' ').slice(0, 160))}`);
    else if (!r.wayOut) fail(`${v.w}px ${route}: says the workflow is gone and offers no way out`);
    else ok(`${v.w}px ${route}: an unknown id reads as an absence, with a way out`);
  }
}

// ============ 6. Nothing decisive sits under the FIXED bottom nav. Arithmetic cannot see this: the
// phone harness reported 0 unreachable pixels with 6px of `start-search` under the bar.
await setVw(WIDTHS[0]);
for (const c of [
  { route: '/initiatives/p2/arena?phaseId=04-leases&step=prompt', controls: ['start-search'] },
  { route: '/workflows/w2/arena', controls: ['promote', 'reject', 'read-the-work'] },
  { route: '/workflows', controls: ['wf-arena-w2'] },
]) {
  await go(c.route);
  const r = await page.evaluate((ids) => {
    const sc = document.querySelector('ion-content')?.shadowRoot?.querySelector('.inner-scroll');
    if (sc) sc.scrollTop = 1e6;
    const nav = document.querySelector('nav.botnav');
    const navBox = nav && (nav.checkVisibility?.({ checkVisibilityCSS: true }) ?? true) ? nav.getBoundingClientRect() : null;
    const out = [];
    for (const id of ids) {
      const el = document.querySelector(`[data-test="${id}"]`);
      if (!el) continue;
      el.scrollIntoView({ block: 'center' });
      const b = el.getBoundingClientRect();
      // The control's own bottom edge, hit-tested. An overlay that covers it answers here and the
      // arithmetic never notices.
      const hit = document.elementFromPoint(b.left + b.width / 2, Math.min(b.bottom - 1, innerHeight - 1));
      out.push({
        id,
        under: navBox ? Math.max(0, Math.round(b.bottom - navBox.top)) : 0,
        edgeHits: hit ? `${hit.tagName.toLowerCase()}${hit.className ? '.' + String(hit.className).split(' ')[0] : ''}` : null,
        mine: !!hit && (hit === el || el.contains(hit)),
      });
    }
    return { navTop: navBox ? Math.round(navBox.top) : null, controls: out };
  }, c.controls);
  for (const ct of r.controls) {
    if (ct.under > 0 || !ct.mine) fail(`${c.route}: [data-test="${ct.id}"] has ${ct.under}px under the fixed bottom nav (nav top ${r.navTop}) and its bottom edge hits ${ct.edgeHits} — occluded, however the scroll arithmetic reads`);
    else ok(`${c.route}: [data-test="${ct.id}"] is clear of the fixed bottom nav`);
  }
  if (!r.controls.length) fail(`${c.route}: none of ${JSON.stringify(c.controls)} is on the page, so nothing was measured`);
}

// ============ 7. The desktop switch is not a layout jump. Reported either way — a number here is
// worth more than a pass/fail, because what is NOT fixed is recorded in the same place.
await setVw(WIDTHS[1]);
const geom = {};
for (const route of ['/initiatives', '/workflows', '/workflows/w2/arena']) {
  await go(route);
  geom[route] = await page.evaluate(() => {
    const box = (s) => { const e = document.querySelector(s); if (!e) return null; const b = e.getBoundingClientRect(); return { x: Math.round(b.x), w: Math.round(b.width) }; };
    return { column: box('.console-center') ?? box('.aw'), list: box('.console-list'), right: box('.console-right') };
  });
}
const consoleCol = geom['/initiatives'].column;
for (const route of ['/workflows', '/workflows/w2/arena']) {
  const col = geom[route].column;
  const dx = Math.abs(col.x - consoleCol.x);
  const dw = Math.abs(col.w - consoleCol.w);
  if (dx <= 8 && dw <= 8) ok(`1440px ${route}: the reading column stays at x=${col.x} w=${col.w}, matching the console's centre pane`);
  else fail(`1440px ${route}: the reading column jumps to x=${col.x} w=${col.w} from the console's x=${consoleCol.x} w=${consoleCol.w} — ${dx}px sideways and ${dw}px wider`);
}
console.log(`  note 1440px: the console's list (${consoleCol ? geom['/initiatives'].list?.w : '?'}px) and validation column (${geom['/initiatives'].right?.w}px) are NOT rendered on arena routes — ${(geom['/initiatives'].list?.w ?? 0) + (geom['/initiatives'].right?.w ?? 0)}px of persistent chrome, deliberately out of scope here (see the report).`);

await browser.close();
server.close();
if (failures.length) {
  console.error(`\n!! ${failures.length} NAV FAILURE(S) !!`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`\nall ${log.length} nav checks pass at both widths: the global nav has an arena entry, the lit item names where you are, the chrome does not change, back survives a reload, a stale link is an absence, and nothing decisive hides under the bottom bar`);
